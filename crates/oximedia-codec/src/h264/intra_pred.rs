//! H.264 intra prediction.
//!
//! For an intra-coded macroblock, the decoder builds the prediction
//! from samples already decoded in the *current frame* — the row above
//! and the column to the left of the block being predicted, plus the
//! top-left corner sample and (for some 4×4 modes) the four samples
//! above-and-right of the current 4×4 block.
//!
//! Three intra paths are covered:
//!
//! - **4×4 luma** — nine modes used by `I_NxN` macroblocks.
//! - **16×16 luma** — four modes (Vertical, Horizontal, DC, Plane)
//!   used by `I_16x16` macroblocks.
//! - **Chroma 8×8 (4:2:0)** — four modes (DC, Horizontal, Vertical,
//!   Plane) shared by all intra-coded macroblocks for the Cb / Cr
//!   components.
//!
//! All paths handle neighbour-availability fallbacks: where a required
//! neighbour sample isn't reconstructed yet (slice boundary, picture
//! edge, constrained-intra-pred), the modes fall back to the spec
//! defaults — typically a constant of `128` or a derived DC from
//! whatever neighbours *are* present.

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

// ---------------------------------------------------------------------------
// 16×16 luma intra prediction (used by I_16x16 macroblocks)
// ---------------------------------------------------------------------------

/// Neighbour samples for one 16×16 luma intra prediction.
#[derive(Debug, Clone, Copy, Default)]
pub struct Intra16x16Neighbours {
    /// Top-left corner sample (one pixel diagonally up-left of `(0, 0)`).
    pub top_left: Option<u8>,
    /// Top neighbour row — 16 samples.
    pub top: Option<[u8; 16]>,
    /// Left neighbour column — 16 samples.
    pub left: Option<[u8; 16]>,
}

/// The four 16×16 luma intra prediction modes from the macroblock layer.
pub use crate::h264::macroblock::Intra16x16PredMode;

/// Predicts a 16×16 luma block under the given mode.
#[must_use]
pub fn predict_16x16(
    mode: Intra16x16PredMode,
    n: &Intra16x16Neighbours,
) -> [[u8; 16]; 16] {
    match mode {
        Intra16x16PredMode::Vertical => predict_16x16_vertical(n),
        Intra16x16PredMode::Horizontal => predict_16x16_horizontal(n),
        Intra16x16PredMode::Dc => predict_16x16_dc(n),
        Intra16x16PredMode::Plane => predict_16x16_plane(n),
    }
}

fn predict_16x16_vertical(n: &Intra16x16Neighbours) -> [[u8; 16]; 16] {
    let top = match n.top {
        Some(t) => t,
        None => return [[128u8; 16]; 16],
    };
    let mut out = [[0u8; 16]; 16];
    for row in &mut out {
        *row = top;
    }
    out
}

fn predict_16x16_horizontal(n: &Intra16x16Neighbours) -> [[u8; 16]; 16] {
    let left = match n.left {
        Some(l) => l,
        None => return [[128u8; 16]; 16],
    };
    let mut out = [[0u8; 16]; 16];
    for (y, row) in out.iter_mut().enumerate() {
        *row = [left[y]; 16];
    }
    out
}

fn predict_16x16_dc(n: &Intra16x16Neighbours) -> [[u8; 16]; 16] {
    let dc = match (n.top, n.left) {
        (Some(t), Some(l)) => {
            let sum: u32 = t.iter().chain(l.iter()).map(|&v| u32::from(v)).sum();
            ((sum + 16) >> 5) as u8
        }
        (Some(t), None) => {
            let sum: u32 = t.iter().map(|&v| u32::from(v)).sum();
            ((sum + 8) >> 4) as u8
        }
        (None, Some(l)) => {
            let sum: u32 = l.iter().map(|&v| u32::from(v)).sum();
            ((sum + 8) >> 4) as u8
        }
        (None, None) => 128,
    };
    [[dc; 16]; 16]
}

fn predict_16x16_plane(n: &Intra16x16Neighbours) -> [[u8; 16]; 16] {
    let (top, left, tl) = match (n.top, n.left, n.top_left) {
        (Some(t), Some(l), Some(p)) => (t, l, p),
        _ => return [[128u8; 16]; 16],
    };
    // H = Σ_{i=0..7} (i+1) * (top[8+i] - top[6-i])   ; at i=7, top[6-i] = top[-1] = top_left
    // V = Σ_{j=0..7} (j+1) * (left[8+j] - left[6-j]) ; at j=7, left[6-j] = left[-1] = top_left
    let mut h_sum: i32 = 0;
    for i in 0..8 {
        let right = i32::from(top[8 + i]);
        let leftward = if i == 7 {
            i32::from(tl)
        } else {
            i32::from(top[6 - i])
        };
        h_sum += (i as i32 + 1) * (right - leftward);
    }
    let mut v_sum: i32 = 0;
    for j in 0..8 {
        let down = i32::from(left[8 + j]);
        let upward = if j == 7 {
            i32::from(tl)
        } else {
            i32::from(left[6 - j])
        };
        v_sum += (j as i32 + 1) * (down - upward);
    }
    let b = (5 * h_sum + 32) >> 6;
    let c = (5 * v_sum + 32) >> 6;
    let a = 16 * (i32::from(left[15]) + i32::from(top[15]));
    let mut out = [[0u8; 16]; 16];
    for y in 0..16 {
        for x in 0..16 {
            let v = (a + b * (x as i32 - 7) + c * (y as i32 - 7) + 16) >> 5;
            out[y][x] = v.clamp(0, 255) as u8;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Chroma 8×8 intra prediction (4:2:0)
// ---------------------------------------------------------------------------

/// Neighbour samples for one 8×8 chroma intra prediction (per component).
#[derive(Debug, Clone, Copy, Default)]
pub struct ChromaIntra8x8Neighbours {
    /// Top-left corner chroma sample.
    pub top_left: Option<u8>,
    /// Top neighbour row — 8 samples.
    pub top: Option<[u8; 8]>,
    /// Left neighbour column — 8 samples.
    pub left: Option<[u8; 8]>,
}

/// The four 8×8 chroma intra prediction modes from the macroblock layer.
pub use crate::h264::macroblock::IntraChromaPredMode;

/// Predicts an 8×8 chroma block under the given mode.
#[must_use]
pub fn predict_chroma_8x8(
    mode: IntraChromaPredMode,
    n: &ChromaIntra8x8Neighbours,
) -> [[u8; 8]; 8] {
    match mode {
        IntraChromaPredMode::Dc => predict_chroma_dc(n),
        IntraChromaPredMode::Horizontal => predict_chroma_horizontal(n),
        IntraChromaPredMode::Vertical => predict_chroma_vertical(n),
        IntraChromaPredMode::Plane => predict_chroma_plane(n),
    }
}

fn predict_chroma_vertical(n: &ChromaIntra8x8Neighbours) -> [[u8; 8]; 8] {
    let top = match n.top {
        Some(t) => t,
        None => return [[128u8; 8]; 8],
    };
    let mut out = [[0u8; 8]; 8];
    for row in &mut out {
        *row = top;
    }
    out
}

fn predict_chroma_horizontal(n: &ChromaIntra8x8Neighbours) -> [[u8; 8]; 8] {
    let left = match n.left {
        Some(l) => l,
        None => return [[128u8; 8]; 8],
    };
    let mut out = [[0u8; 8]; 8];
    for (y, row) in out.iter_mut().enumerate() {
        *row = [left[y]; 8];
    }
    out
}

/// Chroma DC is special: the 8×8 block is split into four 4×4
/// quadrants, each averaging a different subset of the neighbour
/// samples.  Quadrants 0 and 3 (top-left and bottom-right of the
/// chroma block) take both top and left contributions; quadrants 1
/// and 2 take one preferred neighbour subset and fall back to the
/// other when their preferred is absent.
fn predict_chroma_dc(n: &ChromaIntra8x8Neighbours) -> [[u8; 8]; 8] {
    let top_a = n.top.map(|t| sum_u8(&t[..4]));
    let top_b = n.top.map(|t| sum_u8(&t[4..]));
    let left_a = n.left.map(|l| sum_u8(&l[..4]));
    let left_b = n.left.map(|l| sum_u8(&l[4..]));

    let q_tl = dc_average_4_4(top_a, left_a);
    let q_tr = dc_average_prefer_top(top_b, left_a);
    let q_bl = dc_average_prefer_left(left_b, top_a);
    let q_br = dc_average_4_4(top_b, left_b);

    let mut out = [[0u8; 8]; 8];
    for y in 0..4 {
        for x in 0..4 {
            out[y][x] = q_tl;
            out[y][x + 4] = q_tr;
            out[y + 4][x] = q_bl;
            out[y + 4][x + 4] = q_br;
        }
    }
    out
}

fn sum_u8(samples: &[u8]) -> u32 {
    samples.iter().map(|&v| u32::from(v)).sum()
}

fn dc_average_4_4(sum_top: Option<u32>, sum_left: Option<u32>) -> u8 {
    match (sum_top, sum_left) {
        (Some(t), Some(l)) => ((t + l + 4) >> 3) as u8,
        (Some(t), None) => ((t + 2) >> 2) as u8,
        (None, Some(l)) => ((l + 2) >> 2) as u8,
        (None, None) => 128,
    }
}

fn dc_average_prefer_top(sum_top: Option<u32>, sum_left_fallback: Option<u32>) -> u8 {
    if let Some(t) = sum_top {
        ((t + 2) >> 2) as u8
    } else if let Some(l) = sum_left_fallback {
        ((l + 2) >> 2) as u8
    } else {
        128
    }
}

fn dc_average_prefer_left(sum_left: Option<u32>, sum_top_fallback: Option<u32>) -> u8 {
    if let Some(l) = sum_left {
        ((l + 2) >> 2) as u8
    } else if let Some(t) = sum_top_fallback {
        ((t + 2) >> 2) as u8
    } else {
        128
    }
}

fn predict_chroma_plane(n: &ChromaIntra8x8Neighbours) -> [[u8; 8]; 8] {
    let (top, left, tl) = match (n.top, n.left, n.top_left) {
        (Some(t), Some(l), Some(p)) => (t, l, p),
        _ => return [[128u8; 8]; 8],
    };
    let mut h_sum: i32 = 0;
    for i in 0..4 {
        let right = i32::from(top[4 + i]);
        let leftward = if i == 3 {
            i32::from(tl)
        } else {
            i32::from(top[2 - i])
        };
        h_sum += (i as i32 + 1) * (right - leftward);
    }
    let mut v_sum: i32 = 0;
    for j in 0..4 {
        let down = i32::from(left[4 + j]);
        let upward = if j == 3 {
            i32::from(tl)
        } else {
            i32::from(left[2 - j])
        };
        v_sum += (j as i32 + 1) * (down - upward);
    }
    let b = (34 * h_sum + 32) >> 6;
    let c = (34 * v_sum + 32) >> 6;
    let a = 16 * (i32::from(left[7]) + i32::from(top[7]));
    let mut out = [[0u8; 8]; 8];
    for y in 0..8 {
        for x in 0..8 {
            let v = (a + b * (x as i32 - 3) + c * (y as i32 - 3) + 16) >> 5;
            out[y][x] = v.clamp(0, 255) as u8;
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

    // -- 16×16 luma intra tests --

    fn neighbours_16(top_val: u8, left_val: u8, tl: u8) -> Intra16x16Neighbours {
        Intra16x16Neighbours {
            top_left: Some(tl),
            top: Some([top_val; 16]),
            left: Some([left_val; 16]),
        }
    }

    #[test]
    fn intra_16x16_vertical_replicates_top_row() {
        let mut top = [0u8; 16];
        for (i, v) in top.iter_mut().enumerate() {
            *v = i as u8 * 10;
        }
        let n = Intra16x16Neighbours {
            top_left: Some(0),
            top: Some(top),
            left: Some([99u8; 16]),
        };
        let pred = predict_16x16(Intra16x16PredMode::Vertical, &n);
        for y in 0..16 {
            assert_eq!(pred[y], top, "row {y}");
        }
    }

    #[test]
    fn intra_16x16_horizontal_replicates_left_column() {
        let mut left = [0u8; 16];
        for (j, v) in left.iter_mut().enumerate() {
            *v = (j as u8) * 5;
        }
        let n = Intra16x16Neighbours {
            top_left: Some(0),
            top: Some([99u8; 16]),
            left: Some(left),
        };
        let pred = predict_16x16(Intra16x16PredMode::Horizontal, &n);
        for y in 0..16 {
            assert_eq!(pred[y], [left[y]; 16]);
        }
    }

    #[test]
    fn intra_16x16_dc_with_constant_neighbours() {
        let n = neighbours_16(64, 64, 64);
        let pred = predict_16x16(Intra16x16PredMode::Dc, &n);
        assert_eq!(pred, [[64u8; 16]; 16]);
    }

    #[test]
    fn intra_16x16_dc_fallbacks() {
        let only_top = Intra16x16Neighbours {
            top_left: None,
            top: Some([16u8; 16]),
            left: None,
        };
        assert_eq!(
            predict_16x16(Intra16x16PredMode::Dc, &only_top),
            [[16u8; 16]; 16],
        );
        let only_left = Intra16x16Neighbours {
            top_left: None,
            top: None,
            left: Some([32u8; 16]),
        };
        assert_eq!(
            predict_16x16(Intra16x16PredMode::Dc, &only_left),
            [[32u8; 16]; 16],
        );
        let neither = Intra16x16Neighbours::default();
        assert_eq!(
            predict_16x16(Intra16x16PredMode::Dc, &neither),
            [[128u8; 16]; 16],
        );
    }

    #[test]
    fn intra_16x16_plane_constant_input_is_identity() {
        let n = neighbours_16(100, 100, 100);
        let pred = predict_16x16(Intra16x16PredMode::Plane, &n);
        assert_eq!(pred, [[100u8; 16]; 16]);
    }

    #[test]
    fn intra_16x16_plane_falls_back_without_corner() {
        let n = Intra16x16Neighbours {
            top_left: None,
            top: Some([100u8; 16]),
            left: Some([100u8; 16]),
        };
        assert_eq!(
            predict_16x16(Intra16x16PredMode::Plane, &n),
            [[128u8; 16]; 16],
        );
    }

    // -- Chroma 8×8 intra tests --

    fn chroma_neighbours(top_val: u8, left_val: u8, tl: u8) -> ChromaIntra8x8Neighbours {
        ChromaIntra8x8Neighbours {
            top_left: Some(tl),
            top: Some([top_val; 8]),
            left: Some([left_val; 8]),
        }
    }

    #[test]
    fn chroma_vertical_replicates_top() {
        let n = chroma_neighbours(70, 0, 0);
        let pred = predict_chroma_8x8(IntraChromaPredMode::Vertical, &n);
        for row in &pred {
            assert_eq!(*row, [70u8; 8]);
        }
    }

    #[test]
    fn chroma_horizontal_replicates_left() {
        let mut left = [0u8; 8];
        for (j, v) in left.iter_mut().enumerate() {
            *v = j as u8 * 8;
        }
        let n = ChromaIntra8x8Neighbours {
            top_left: Some(0),
            top: Some([99u8; 8]),
            left: Some(left),
        };
        let pred = predict_chroma_8x8(IntraChromaPredMode::Horizontal, &n);
        for y in 0..8 {
            assert_eq!(pred[y], [left[y]; 8]);
        }
    }

    #[test]
    fn chroma_dc_with_all_neighbours_present() {
        let n = chroma_neighbours(80, 80, 80);
        let pred = predict_chroma_8x8(IntraChromaPredMode::Dc, &n);
        for row in &pred {
            assert_eq!(*row, [80u8; 8]);
        }
    }

    #[test]
    fn chroma_dc_top_right_prefers_top_then_left_then_default() {
        // Top present only: TR quadrant uses top[4..7], all others
        // fall back per spec.
        let top_only = ChromaIntra8x8Neighbours {
            top_left: None,
            top: Some([40u8; 8]),
            left: None,
        };
        let pred = predict_chroma_8x8(IntraChromaPredMode::Dc, &top_only);
        // All quadrants should be 40 when top is uniform 40.
        for row in &pred {
            assert_eq!(*row, [40u8; 8]);
        }

        // Left present only: TR falls back to left[0..3].
        let left_only = ChromaIntra8x8Neighbours {
            top_left: None,
            top: None,
            left: Some([50u8; 8]),
        };
        let pred = predict_chroma_8x8(IntraChromaPredMode::Dc, &left_only);
        for row in &pred {
            assert_eq!(*row, [50u8; 8]);
        }

        // Neither: 128.
        let neither = ChromaIntra8x8Neighbours::default();
        let pred = predict_chroma_8x8(IntraChromaPredMode::Dc, &neither);
        for row in &pred {
            assert_eq!(*row, [128u8; 8]);
        }
    }

    #[test]
    fn chroma_plane_constant_input_is_identity() {
        let n = chroma_neighbours(50, 50, 50);
        let pred = predict_chroma_8x8(IntraChromaPredMode::Plane, &n);
        for row in &pred {
            assert_eq!(*row, [50u8; 8]);
        }
    }

    #[test]
    fn chroma_plane_falls_back_without_corner() {
        let n = ChromaIntra8x8Neighbours {
            top_left: None,
            top: Some([90u8; 8]),
            left: Some([90u8; 8]),
        };
        let pred = predict_chroma_8x8(IntraChromaPredMode::Plane, &n);
        for row in &pred {
            assert_eq!(*row, [128u8; 8]);
        }
    }
}
