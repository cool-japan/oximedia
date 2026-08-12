//! VP9 intra prediction — exact port of libvpx `vp9/common/vp9_reconintra.c`
//! (border construction) and `vpx_dsp/intrapred.c` (predictor kernels),
//! 8-bit build.
//!
//! libvpx dispatches the six directional modes to special hand-written 4x4
//! kernels (`vpx_d45_predictor_4x4_c`, ...) and to generic `bs`-parameterized
//! kernels for 8/16/32 (`intra_pred_no_4x4`); V/H/TM/DC use the generic
//! kernels for all sizes. That split is reproduced here — it is required for
//! bit-exactness (e.g. generic D45 fills the last row-0 pixel with
//! `above[bs-1]`, while the 4x4 kernel reads real above-right pixels).

#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_range_loop)]

/// VP9 intra prediction modes (spec order: DC=0 .. TM=9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PredMode {
    /// Average of above/left.
    #[default]
    Dc = 0,
    /// Vertical.
    V = 1,
    /// Horizontal.
    H = 2,
    /// Diagonal 45 degrees.
    D45 = 3,
    /// Diagonal 135 degrees.
    D135 = 4,
    /// Diagonal 117 degrees.
    D117 = 5,
    /// Diagonal 153 degrees.
    D153 = 6,
    /// Diagonal 207 degrees.
    D207 = 7,
    /// Diagonal 63 degrees.
    D63 = 8,
    /// True-motion.
    Tm = 9,
}

impl PredMode {
    /// Mode from its VP9 integer value (0..=9); values are produced by the
    /// intra-mode tree decode and are always in range.
    #[must_use]
    pub fn from_index(v: u8) -> Self {
        match v {
            1 => Self::V,
            2 => Self::H,
            3 => Self::D45,
            4 => Self::D135,
            5 => Self::D117,
            6 => Self::D153,
            7 => Self::D207,
            8 => Self::D63,
            9 => Self::Tm,
            _ => Self::Dc,
        }
    }
}

/// libvpx `extend_modes` NEED_* flags.
const NEED_LEFT: u8 = 1 << 1;
const NEED_ABOVE: u8 = 1 << 2;
const NEED_ABOVERIGHT: u8 = 1 << 3;

const EXTEND_MODES: [u8; 10] = [
    NEED_ABOVE | NEED_LEFT, // DC
    NEED_ABOVE,             // V
    NEED_LEFT,              // H
    NEED_ABOVERIGHT,        // D45
    NEED_LEFT | NEED_ABOVE, // D135
    NEED_LEFT | NEED_ABOVE, // D117
    NEED_LEFT | NEED_ABOVE, // D153
    NEED_LEFT,              // D207
    NEED_ABOVERIGHT,        // D63
    NEED_LEFT | NEED_ABOVE, // TM
];

#[inline]
fn avg2(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b) + 1) >> 1) as u8
}

#[inline]
fn avg3(a: u8, b: u8, c: u8) -> u8 {
    ((u16::from(a) + 2 * u16::from(b) + u16::from(c) + 2) >> 2) as u8
}

/// Neighbor data for one tx block: `corner` = `above_row[-1]`,
/// `above[0..2*bs]`, `left[0..bs]`.
struct Border {
    corner: u8,
    above: [u8; 64],
    left: [u8; 32],
}

/// Builds the above/left borders exactly as libvpx `build_intra_predictors`.
///
/// * `buf`/`stride`: reconstruction plane (MI-aligned dimensions).
/// * `x0`, `y0`: tx block position in plane pixels.
/// * `bs`: transform block size in pixels (4/8/16/32).
/// * `up_available` / `left_available` / `right_available`: as computed by
///   `vp9_predict_intra_block`.
/// * `edge_slow_x` / `edge_slow_y`: `mb_to_right_edge < 0` /
///   `mb_to_bottom_edge < 0` for the enclosing prediction block.
/// * `frame_w` / `frame_h`: plane dimensions of the MI-aligned frame
///   (libvpx `cur_buf->y_width` is the 8-aligned width).
fn build_border(
    buf: &[u8],
    stride: usize,
    x0: usize,
    y0: usize,
    bs: usize,
    mode: PredMode,
    up_available: bool,
    left_available: bool,
    right_available: bool,
    edge_slow_x: bool,
    edge_slow_y: bool,
    frame_w: usize,
    frame_h: usize,
) -> Border {
    let mut b = Border {
        corner: 129,
        above: [0; 64],
        left: [0; 32],
    };
    let need = EXTEND_MODES[mode as usize];
    let o = y0 * stride + x0;

    // NEED_LEFT
    if need & NEED_LEFT != 0 || mode == PredMode::Dc {
        if left_available {
            if edge_slow_y {
                if y0 + bs <= frame_h {
                    for i in 0..bs {
                        b.left[i] = buf[o + i * stride - 1];
                    }
                } else {
                    let extend_bottom = frame_h - y0;
                    for i in 0..extend_bottom {
                        b.left[i] = buf[o + i * stride - 1];
                    }
                    let last = buf[o + (extend_bottom - 1) * stride - 1];
                    for i in extend_bottom..bs {
                        b.left[i] = last;
                    }
                }
            } else {
                for i in 0..bs {
                    b.left[i] = buf[o + i * stride - 1];
                }
            }
        } else {
            b.left[..bs].fill(129);
        }
    }

    // NEED_ABOVE
    if need & NEED_ABOVE != 0 || mode == PredMode::Dc {
        if up_available {
            let a = o - stride; // above_ref
            if edge_slow_x {
                if x0 + bs <= frame_w {
                    b.above[..bs].copy_from_slice(&buf[a..a + bs]);
                } else if x0 <= frame_w {
                    let r = frame_w - x0;
                    b.above[..r].copy_from_slice(&buf[a..a + r]);
                    let last = b.above[r - 1];
                    b.above[r..bs].fill(last);
                }
            } else {
                b.above[..bs].copy_from_slice(&buf[a..a + bs]);
            }
            b.corner = if left_available { buf[a - 1] } else { 129 };
        } else {
            b.above[..bs].fill(127);
            b.corner = 127;
        }
    }

    // NEED_ABOVERIGHT
    if need & NEED_ABOVERIGHT != 0 {
        if up_available {
            let a = o - stride; // above_ref
            if edge_slow_x {
                if x0 + 2 * bs <= frame_w {
                    if right_available && bs == 4 {
                        b.above[..2 * bs].copy_from_slice(&buf[a..a + 2 * bs]);
                    } else {
                        b.above[..bs].copy_from_slice(&buf[a..a + bs]);
                        let last = b.above[bs - 1];
                        b.above[bs..2 * bs].fill(last);
                    }
                } else if x0 + bs <= frame_w {
                    let r = frame_w - x0;
                    if right_available && bs == 4 {
                        b.above[..r].copy_from_slice(&buf[a..a + r]);
                        let last = b.above[r - 1];
                        b.above[r..2 * bs].fill(last);
                    } else {
                        b.above[..bs].copy_from_slice(&buf[a..a + bs]);
                        let last = b.above[bs - 1];
                        b.above[bs..2 * bs].fill(last);
                    }
                } else if x0 <= frame_w {
                    let r = frame_w - x0;
                    b.above[..r].copy_from_slice(&buf[a..a + r]);
                    let last = b.above[r - 1];
                    b.above[r..2 * bs].fill(last);
                }
                b.corner = if left_available { buf[a - 1] } else { 129 };
            } else {
                // Fast path: the libvpx aliasing branches read the same
                // pixels this copy captures (predictors only touch
                // above[-1 .. 2*bs)).
                if bs == 4 && right_available {
                    b.above[..2 * bs].copy_from_slice(&buf[a..a + 2 * bs]);
                } else {
                    b.above[..bs].copy_from_slice(&buf[a..a + bs]);
                    let last = b.above[bs - 1];
                    b.above[bs..2 * bs].fill(last);
                }
                b.corner = if left_available { buf[a - 1] } else { 129 };
            }
        } else {
            b.above[..2 * bs].fill(127);
            b.corner = 127;
        }
    }

    b
}

/// Predicts one intra tx block in place (`vp9_predict_intra_block` +
/// `build_intra_predictors` + kernel dispatch).
pub fn predict_intra(
    buf: &mut [u8],
    stride: usize,
    x0: usize,
    y0: usize,
    bs: usize,
    mode: PredMode,
    up_available: bool,
    left_available: bool,
    right_available: bool,
    edge_slow_x: bool,
    edge_slow_y: bool,
    frame_w: usize,
    frame_h: usize,
) {
    let border = build_border(
        buf,
        stride,
        x0,
        y0,
        bs,
        mode,
        up_available,
        left_available,
        right_available,
        edge_slow_x,
        edge_slow_y,
        frame_w,
        frame_h,
    );
    let o = y0 * stride + x0;
    let (corner, above, left) = (border.corner, &border.above, &border.left);

    match mode {
        PredMode::Dc => match (left_available, up_available) {
            (false, false) => fill_block(buf, o, stride, bs, 128),
            (false, true) => {
                let sum: u32 = above[..bs].iter().map(|&v| u32::from(v)).sum();
                let dc = ((sum + (bs as u32 >> 1)) / bs as u32) as u8;
                fill_block(buf, o, stride, bs, dc);
            }
            (true, false) => {
                let sum: u32 = left[..bs].iter().map(|&v| u32::from(v)).sum();
                let dc = ((sum + (bs as u32 >> 1)) / bs as u32) as u8;
                fill_block(buf, o, stride, bs, dc);
            }
            (true, true) => {
                let sum: u32 = above[..bs]
                    .iter()
                    .chain(left[..bs].iter())
                    .map(|&v| u32::from(v))
                    .sum();
                let count = 2 * bs as u32;
                let dc = ((sum + (count >> 1)) / count) as u8;
                fill_block(buf, o, stride, bs, dc);
            }
        },
        PredMode::V => {
            for r in 0..bs {
                buf[o + r * stride..o + r * stride + bs].copy_from_slice(&above[..bs]);
            }
        }
        PredMode::H => {
            for r in 0..bs {
                buf[o + r * stride..o + r * stride + bs].fill(left[r]);
            }
        }
        PredMode::Tm => {
            let tl = i32::from(corner);
            for r in 0..bs {
                let l = i32::from(left[r]);
                for c in 0..bs {
                    let v = (l + i32::from(above[c]) - tl).clamp(0, 255) as u8;
                    buf[o + r * stride + c] = v;
                }
            }
        }
        PredMode::D45 => {
            if bs == 4 {
                d45_4x4(buf, o, stride, above);
            } else {
                d45(buf, o, stride, bs, above);
            }
        }
        PredMode::D63 => {
            if bs == 4 {
                d63_4x4(buf, o, stride, above);
            } else {
                d63(buf, o, stride, bs, above);
            }
        }
        PredMode::D117 => {
            if bs == 4 {
                d117_4x4(buf, o, stride, corner, above, left);
            } else {
                d117(buf, o, stride, bs, corner, above, left);
            }
        }
        PredMode::D135 => {
            if bs == 4 {
                d135_4x4(buf, o, stride, corner, above, left);
            } else {
                d135(buf, o, stride, bs, corner, above, left);
            }
        }
        PredMode::D153 => {
            if bs == 4 {
                d153_4x4(buf, o, stride, corner, above, left);
            } else {
                d153(buf, o, stride, bs, corner, above, left);
            }
        }
        PredMode::D207 => {
            if bs == 4 {
                d207_4x4(buf, o, stride, left);
            } else {
                d207(buf, o, stride, bs, left);
            }
        }
    }
}

fn fill_block(buf: &mut [u8], o: usize, stride: usize, bs: usize, v: u8) {
    for r in 0..bs {
        buf[o + r * stride..o + r * stride + bs].fill(v);
    }
}

// ---------------------------------------------------------------------------
// Generic (8/16/32) kernels — ports of the `bs`-parameterized libvpx C.
// ---------------------------------------------------------------------------

/// `d207_predictor`.
fn d207(buf: &mut [u8], o: usize, stride: usize, bs: usize, left: &[u8]) {
    // first column
    for r in 0..bs - 1 {
        buf[o + r * stride] = avg2(left[r], left[r + 1]);
    }
    buf[o + (bs - 1) * stride] = left[bs - 1];
    // second column
    for r in 0..bs - 2 {
        buf[o + 1 + r * stride] = avg3(left[r], left[r + 1], left[r + 2]);
    }
    buf[o + 1 + (bs - 2) * stride] = avg3(left[bs - 2], left[bs - 1], left[bs - 1]);
    buf[o + 1 + (bs - 1) * stride] = left[bs - 1];
    // rest of last row
    for c in 0..bs - 2 {
        buf[o + 2 + (bs - 1) * stride + c] = left[bs - 1];
    }
    // rest of the block: dst[r][c] = dst[r+1][c-2] (c relative to col 2)
    for r in (0..bs - 1).rev() {
        for c in 0..bs - 2 {
            buf[o + 2 + r * stride + c] = buf[o + 2 + (r + 1) * stride + c - 2];
        }
    }
}

/// `d63_predictor`.
fn d63(buf: &mut [u8], o: usize, stride: usize, bs: usize, above: &[u8]) {
    for c in 0..bs {
        buf[o + c] = avg2(above[c], above[c + 1]);
        buf[o + stride + c] = avg3(above[c], above[c + 1], above[c + 2]);
    }
    let mut size = bs - 2;
    let mut r = 2;
    while r < bs {
        for c in 0..size {
            buf[o + r * stride + c] = buf[o + (r >> 1) + c];
        }
        for c in size..bs {
            buf[o + r * stride + c] = above[bs - 1];
        }
        for c in 0..size {
            buf[o + (r + 1) * stride + c] = buf[o + stride + (r >> 1) + c];
        }
        for c in size..bs {
            buf[o + (r + 1) * stride + c] = above[bs - 1];
        }
        r += 2;
        size -= 1;
    }
}

/// `d45_predictor`.
fn d45(buf: &mut [u8], o: usize, stride: usize, bs: usize, above: &[u8]) {
    let above_right = above[bs - 1];
    for x in 0..bs - 1 {
        buf[o + x] = avg3(above[x], above[x + 1], above[x + 2]);
    }
    buf[o + bs - 1] = above_right;
    let mut size = bs - 2;
    for x in 1..bs {
        for c in 0..size {
            buf[o + x * stride + c] = buf[o + x + c];
        }
        for c in size..bs {
            buf[o + x * stride + c] = above_right;
        }
        size = size.wrapping_sub(1);
    }
}

/// `d117_predictor`.
fn d117(buf: &mut [u8], o: usize, stride: usize, bs: usize, corner: u8, above: &[u8], left: &[u8]) {
    // first row
    buf[o] = avg2(corner, above[0]);
    for c in 1..bs {
        buf[o + c] = avg2(above[c - 1], above[c]);
    }
    // second row
    buf[o + stride] = avg3(left[0], corner, above[0]);
    buf[o + stride + 1] = avg3(corner, above[0], above[1]);
    for c in 2..bs {
        buf[o + stride + c] = avg3(above[c - 2], above[c - 1], above[c]);
    }
    // the rest of the first column
    buf[o + 2 * stride] = avg3(corner, left[0], left[1]);
    for r in 3..bs {
        buf[o + r * stride] = avg3(left[r - 3], left[r - 2], left[r - 1]);
    }
    // the rest of the block
    for r in 2..bs {
        for c in 1..bs {
            buf[o + r * stride + c] = buf[o + (r - 2) * stride + c - 1];
        }
    }
}

/// `d135_predictor` (border array from bottom-left to top-right).
fn d135(buf: &mut [u8], o: usize, stride: usize, bs: usize, corner: u8, above: &[u8], left: &[u8]) {
    let mut border = [0u8; 32 + 32 - 1];
    for i in 0..bs - 2 {
        border[i] = avg3(left[bs - 3 - i], left[bs - 2 - i], left[bs - 1 - i]);
    }
    border[bs - 2] = avg3(corner, left[0], left[1]);
    border[bs - 1] = avg3(left[0], corner, above[0]);
    border[bs] = avg3(corner, above[0], above[1]);
    for i in 0..bs - 2 {
        border[bs + 1 + i] = avg3(above[i], above[i + 1], above[i + 2]);
    }
    for i in 0..bs {
        let src = bs - 1 - i;
        buf[o + i * stride..o + i * stride + bs].copy_from_slice(&border[src..src + bs]);
    }
}

/// `d153_predictor`.
fn d153(buf: &mut [u8], o: usize, stride: usize, bs: usize, corner: u8, above: &[u8], left: &[u8]) {
    // column 0
    buf[o] = avg2(corner, left[0]);
    for r in 1..bs {
        buf[o + r * stride] = avg2(left[r - 1], left[r]);
    }
    // column 1
    buf[o + 1] = avg3(left[0], corner, above[0]);
    buf[o + 1 + stride] = avg3(corner, left[0], left[1]);
    for r in 2..bs {
        buf[o + 1 + r * stride] = avg3(left[r - 2], left[r - 1], left[r]);
    }
    // row 0, columns 2..
    buf[o + 2] = avg3(corner, above[0], above[1]);
    for c in 1..bs - 2 {
        buf[o + 2 + c] = avg3(above[c - 1], above[c], above[c + 1]);
    }
    // the rest
    for r in 1..bs {
        for c in 0..bs - 2 {
            buf[o + 2 + r * stride + c] = buf[o + 2 + (r - 1) * stride + c - 2];
        }
    }
}

// ---------------------------------------------------------------------------
// Special 4x4 kernels (`vpx_*_predictor_4x4_c`) — VP8-style formulas.
// ---------------------------------------------------------------------------

fn d207_4x4(buf: &mut [u8], o: usize, stride: usize, left: &[u8]) {
    let (i, j, k, l) = (left[0], left[1], left[2], left[3]);
    let mut set = |x: usize, y: usize, v: u8| buf[o + y * stride + x] = v;
    set(0, 0, avg2(i, j));
    let v = avg2(j, k);
    set(2, 0, v);
    set(0, 1, v);
    let v = avg2(k, l);
    set(2, 1, v);
    set(0, 2, v);
    set(1, 0, avg3(i, j, k));
    let v = avg3(j, k, l);
    set(3, 0, v);
    set(1, 1, v);
    let v = avg3(k, l, l);
    set(3, 1, v);
    set(1, 2, v);
    set(3, 2, l);
    set(2, 2, l);
    set(0, 3, l);
    set(1, 3, l);
    set(2, 3, l);
    set(3, 3, l);
}

fn d63_4x4(buf: &mut [u8], o: usize, stride: usize, above: &[u8]) {
    let (a, b, c, d, e, f, g) = (
        above[0], above[1], above[2], above[3], above[4], above[5], above[6],
    );
    let mut set = |x: usize, y: usize, v: u8| buf[o + y * stride + x] = v;
    set(0, 0, avg2(a, b));
    let v = avg2(b, c);
    set(1, 0, v);
    set(0, 2, v);
    let v = avg2(c, d);
    set(2, 0, v);
    set(1, 2, v);
    let v = avg2(d, e);
    set(3, 0, v);
    set(2, 2, v);
    set(3, 2, avg2(e, f)); // differs from vp8
    set(0, 1, avg3(a, b, c));
    let v = avg3(b, c, d);
    set(1, 1, v);
    set(0, 3, v);
    let v = avg3(c, d, e);
    set(2, 1, v);
    set(1, 3, v);
    let v = avg3(d, e, f);
    set(3, 1, v);
    set(2, 3, v);
    set(3, 3, avg3(e, f, g)); // differs from vp8
}

fn d45_4x4(buf: &mut [u8], o: usize, stride: usize, above: &[u8]) {
    let (a, b, c, d, e, f, g, h) = (
        above[0], above[1], above[2], above[3], above[4], above[5], above[6], above[7],
    );
    let mut set = |x: usize, y: usize, v: u8| buf[o + y * stride + x] = v;
    set(0, 0, avg3(a, b, c));
    let v = avg3(b, c, d);
    set(1, 0, v);
    set(0, 1, v);
    let v = avg3(c, d, e);
    set(2, 0, v);
    set(1, 1, v);
    set(0, 2, v);
    let v = avg3(d, e, f);
    set(3, 0, v);
    set(2, 1, v);
    set(1, 2, v);
    set(0, 3, v);
    let v = avg3(e, f, g);
    set(3, 1, v);
    set(2, 2, v);
    set(1, 3, v);
    let v = avg3(f, g, h);
    set(3, 2, v);
    set(2, 3, v);
    set(3, 3, h); // differs from vp8
}

fn d117_4x4(buf: &mut [u8], o: usize, stride: usize, corner: u8, above: &[u8], left: &[u8]) {
    let (i, j, k) = (left[0], left[1], left[2]);
    let x = corner;
    let (a, b, c, d) = (above[0], above[1], above[2], above[3]);
    let mut set = |px: usize, py: usize, v: u8| buf[o + py * stride + px] = v;
    let v = avg2(x, a);
    set(0, 0, v);
    set(1, 2, v);
    let v = avg2(a, b);
    set(1, 0, v);
    set(2, 2, v);
    let v = avg2(b, c);
    set(2, 0, v);
    set(3, 2, v);
    set(3, 0, avg2(c, d));
    set(0, 3, avg3(k, j, i));
    set(0, 2, avg3(j, i, x));
    let v = avg3(i, x, a);
    set(0, 1, v);
    set(1, 3, v);
    let v = avg3(x, a, b);
    set(1, 1, v);
    set(2, 3, v);
    let v = avg3(a, b, c);
    set(2, 1, v);
    set(3, 3, v);
    set(3, 1, avg3(b, c, d));
}

fn d135_4x4(buf: &mut [u8], o: usize, stride: usize, corner: u8, above: &[u8], left: &[u8]) {
    let (i, j, k, l) = (left[0], left[1], left[2], left[3]);
    let x = corner;
    let (a, b, c, d) = (above[0], above[1], above[2], above[3]);
    let mut set = |px: usize, py: usize, v: u8| buf[o + py * stride + px] = v;
    set(0, 3, avg3(j, k, l));
    let v = avg3(i, j, k);
    set(1, 3, v);
    set(0, 2, v);
    let v = avg3(x, i, j);
    set(2, 3, v);
    set(1, 2, v);
    set(0, 1, v);
    let v = avg3(a, x, i);
    set(3, 3, v);
    set(2, 2, v);
    set(1, 1, v);
    set(0, 0, v);
    let v = avg3(b, a, x);
    set(3, 2, v);
    set(2, 1, v);
    set(1, 0, v);
    let v = avg3(c, b, a);
    set(3, 1, v);
    set(2, 0, v);
    set(3, 0, avg3(d, c, b));
}

fn d153_4x4(buf: &mut [u8], o: usize, stride: usize, corner: u8, above: &[u8], left: &[u8]) {
    let (i, j, k, l) = (left[0], left[1], left[2], left[3]);
    let x = corner;
    let (a, b, c) = (above[0], above[1], above[2]);
    let mut set = |px: usize, py: usize, v: u8| buf[o + py * stride + px] = v;
    let v = avg2(i, x);
    set(0, 0, v);
    set(2, 1, v);
    let v = avg2(j, i);
    set(0, 1, v);
    set(2, 2, v);
    let v = avg2(k, j);
    set(0, 2, v);
    set(2, 3, v);
    set(0, 3, avg2(l, k));
    set(3, 0, avg3(a, b, c));
    set(2, 0, avg3(x, a, b));
    let v = avg3(i, x, a);
    set(1, 0, v);
    set(3, 1, v);
    let v = avg3(j, i, x);
    set(1, 1, v);
    set(3, 2, v);
    let v = avg3(k, j, i);
    set(1, 2, v);
    set(3, 3, v);
    set(1, 3, avg3(l, k, j));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_128_when_no_neighbors() {
        let mut buf = vec![0u8; 64];
        predict_intra(
            &mut buf,
            8,
            0,
            0,
            4,
            PredMode::Dc,
            false,
            false,
            false,
            false,
            false,
            8,
            8,
        );
        for r in 0..4 {
            for c in 0..4 {
                assert_eq!(buf[r * 8 + c], 128);
            }
        }
    }

    #[test]
    fn v_pred_copies_above_row() {
        let mut buf = vec![0u8; 64];
        for c in 0..8 {
            buf[c] = 10 + c as u8; // row 0 = above reference for block at y=1
        }
        predict_intra(
            &mut buf,
            8,
            0,
            1,
            4,
            PredMode::V,
            true,
            false,
            false,
            false,
            false,
            8,
            8,
        );
        for r in 1..5 {
            for c in 0..4 {
                assert_eq!(buf[r * 8 + c], 10 + c as u8);
            }
        }
    }

    #[test]
    fn missing_above_is_127_missing_left_is_129() {
        // V with no above -> rows of 127
        let mut buf = vec![0u8; 64];
        predict_intra(
            &mut buf,
            8,
            0,
            0,
            4,
            PredMode::V,
            false,
            false,
            false,
            false,
            false,
            8,
            8,
        );
        assert_eq!(buf[0], 127);
        // H with no left -> rows of 129
        let mut buf2 = vec![0u8; 64];
        predict_intra(
            &mut buf2,
            8,
            0,
            0,
            4,
            PredMode::H,
            false,
            false,
            false,
            false,
            false,
            8,
            8,
        );
        assert_eq!(buf2[0], 129);
    }
}
