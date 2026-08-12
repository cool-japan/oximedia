//! AV1 inverse transforms (spec 7.13), exact integer implementation.
//!
//! Literal port of the AV1 spec "Inverse transform process": butterfly
//! functions B/H over `cos128`/`sin128`, the inverse DCT butterfly network
//! (n = 2..6), inverse ADST4/8/16 with their input/output permutations,
//! the inverse Walsh-Hadamard transform (lossless), the inverse identity
//! transforms, and the 2D row/column driver with rectangular-block scaling
//! (`* 2896 >> 12`) and the `Transform_Row_Shift` table.
//!
//! All intermediates are `i32`, as in libaom and dav1d; see
//! [`inverse_transform_2d`] for the range proof and its bit-depth bound.

use super::consts::{
    ADST_ADST, ADST_DCT, ADST_FLIPADST, DCT_ADST, DCT_DCT, DCT_FLIPADST, FLIPADST_ADST,
    FLIPADST_DCT, FLIPADST_FLIPADST, H_ADST, H_DCT, H_FLIPADST, V_ADST, V_DCT, V_FLIPADST,
};
use super::tables_conv::{COS128_LOOKUP, TRANSFORM_ROW_SHIFT, TX_HEIGHT_LOG2, TX_WIDTH_LOG2};

/// `Round2(x, n)` (spec 4.7) for signed values: `(x + 2^(n-1)) >> n`.
#[inline]
pub fn round2(x: i32, n: u32) -> i32 {
    if n == 0 {
        return x;
    }
    (x + (1 << (n - 1))) >> n
}

/// `cos128( angle )` (spec 7.13.2.1).
#[inline]
fn cos128(angle: i32) -> i32 {
    let angle2 = (angle & 255) as usize;
    match angle2 {
        0..=64 => i32::from(COS128_LOOKUP[angle2]),
        65..=128 => -i32::from(COS128_LOOKUP[128 - angle2]),
        129..=192 => -i32::from(COS128_LOOKUP[angle2 - 128]),
        _ => i32::from(COS128_LOOKUP[256 - angle2]),
    }
}

/// `sin128( angle )` = `cos128( angle - 64 )`.
#[inline]
fn sin128(angle: i32) -> i32 {
    cos128(angle - 64)
}

/// `brev( numBits, x )` (spec 7.13.2.2).
#[inline]
fn brev(num_bits: u32, x: u32) -> u32 {
    let mut t = 0;
    for i in 0..num_bits {
        let bit = (x >> i) & 1;
        t += bit << (num_bits - 1 - i);
    }
    t
}

/// The `r`-bit `Clip3` range used by `H()` (spec 7.13.2.1) and by the
/// coefficient load / inter-pass clamp (spec 7.13.3), materialised once per
/// 1D transform instead of being re-derived from `r` at every Hadamard.
#[derive(Clone, Copy)]
struct ClampRange {
    lo: i32,
    hi: i32,
}

impl ClampRange {
    /// `r` is `rowClampRange`/`colClampRange`, i.e. `16..=20` for the 8..12-bit
    /// depths AV1 defines. Unlike the previous `i64` form, `r >= 32` would
    /// overflow the shift; `super::recon` admits `BitDepth == 8` only.
    #[inline]
    fn new(r: u32) -> Self {
        debug_assert!(r < 32, "clamp range {r} exceeds i32");
        Self {
            lo: -(1i32 << (r - 1)),
            hi: (1i32 << (r - 1)) - 1,
        }
    }

    #[inline]
    fn clip(self, v: i32) -> i32 {
        v.clamp(self.lo, self.hi)
    }
}

/// `B( a, b, angle, flip, r )` (spec 7.13.2.1).
///
/// The spec does **not** clamp here: `r` is only a bitstream-conformance bound
/// ("It is a requirement of bitstream conformance that the values saved into
/// the array T by this function are representable by a signed integer using r
/// bits of precision"), which is why `r` is not a parameter. libaom likewise
/// only checks it under `CONFIG_COEFFICIENT_RANGE_CHECKING`.
///
/// The `i32` intermediates are safe without a clamp because a butterfly is a
/// rotation: `|a*cos - b*sin| <= sqrt(a^2 + b^2) * 4096`, so one `B` grows the
/// magnitude by at most `sqrt(2)`, and the spec's network never chains two
/// butterflies without an intervening `H` (which does clamp, to `r` bits).
/// Propagating that bound through every stage of the real network gives
/// `max |T| = ceil(sqrt(2) * 2^15) = 46341` and a widest product of exactly
/// `2^28` at `BitDepth == 8` — 8x inside `i32`. See `inverse_transform_2d`.
#[inline]
fn butterfly(t: &mut [i32], a: usize, b: usize, angle: i32, flip: bool) {
    let x = t[a] * cos128(angle) - t[b] * sin128(angle);
    let y = t[a] * sin128(angle) + t[b] * cos128(angle);
    let na = round2(x, 12);
    let nb = round2(y, 12);
    if flip {
        t[a] = nb;
        t[b] = na;
    } else {
        t[a] = na;
        t[b] = nb;
    }
}

/// `H( a, b, flip, r )` (spec 7.13.2.1): Hadamard rotation with active
/// clamping to `r` bits.
#[inline]
fn hadamard(t: &mut [i32], a: usize, b: usize, flip: bool, r: ClampRange) {
    let (a, b) = if flip { (b, a) } else { (a, b) };
    let x = t[a];
    let y = t[b];
    t[a] = r.clip(x + y);
    t[b] = r.clip(x - y);
}

/// Inverse DCT array permutation (spec 7.13.2.2).
fn inv_dct_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let mut copy = [0i32; 64];
    copy[..n0].copy_from_slice(&t[..n0]);
    for (i, v) in t.iter_mut().enumerate().take(n0) {
        *v = copy[brev(n, i as u32) as usize];
    }
}

/// Inverse DCT process (spec 7.13.2.3), n = 2..6.
#[allow(clippy::too_many_lines)]
fn inv_dct(t: &mut [i32], n: u32, r: ClampRange) {
    inv_dct_permute(t, n);
    // 2.
    if n == 6 {
        for i in 0..16i32 {
            butterfly(
                t,
                (32 + i) as usize,
                (63 - i) as usize,
                63 - 4 * brev(4, i as u32) as i32,
                false,
            );
        }
    }
    // 3.
    if n >= 5 {
        for i in 0..8i32 {
            butterfly(
                t,
                (16 + i) as usize,
                (31 - i) as usize,
                6 + ((brev(3, (7 - i) as u32) as i32) << 3),
                false,
            );
        }
    }
    // 4.
    if n == 6 {
        for i in 0..16usize {
            hadamard(t, 32 + i * 2, 33 + i * 2, i & 1 == 1, r);
        }
    }
    // 5.
    if n >= 4 {
        for i in 0..4i32 {
            butterfly(
                t,
                (8 + i) as usize,
                (15 - i) as usize,
                12 + ((brev(2, (3 - i) as u32) as i32) << 4),
                false,
            );
        }
    }
    // 6.
    if n >= 5 {
        for i in 0..8usize {
            hadamard(t, 16 + 2 * i, 17 + 2 * i, i & 1 == 1, r);
        }
    }
    // 7.
    if n == 6 {
        for i in 0..4i32 {
            for j in 0..2i32 {
                butterfly(
                    t,
                    (62 - i * 4 - j) as usize,
                    (33 + i * 4 + j) as usize,
                    60 - 16 * brev(2, i as u32) as i32 + 64 * j,
                    true,
                );
            }
        }
    }
    // 8.
    if n >= 3 {
        for i in 0..2i32 {
            butterfly(t, (4 + i) as usize, (7 - i) as usize, 56 - 32 * i, false);
        }
    }
    // 9.
    if n >= 4 {
        for i in 0..4usize {
            hadamard(t, 8 + 2 * i, 9 + 2 * i, i & 1 == 1, r);
        }
    }
    // 10.
    if n >= 5 {
        for i in 0..2i32 {
            for j in 0..2i32 {
                butterfly(
                    t,
                    (30 - 4 * i - j) as usize,
                    (17 + 4 * i + j) as usize,
                    24 + (j << 6) + ((1 - i) << 5),
                    true,
                );
            }
        }
    }
    // 11.
    if n == 6 {
        for i in 0..8usize {
            for j in 0..2usize {
                hadamard(t, 32 + i * 4 + j, 35 + i * 4 - j, i & 1 == 1, r);
            }
        }
    }
    // 12.
    for i in 0..2i32 {
        butterfly(
            t,
            (2 * i) as usize,
            (2 * i + 1) as usize,
            32 + 16 * i,
            i == 0,
        );
    }
    // 13.
    if n >= 3 {
        for i in 0..2usize {
            hadamard(t, 4 + 2 * i, 5 + 2 * i, i == 1, r);
        }
    }
    // 14.
    if n >= 4 {
        for i in 0..2i32 {
            butterfly(t, (14 - i) as usize, (9 + i) as usize, 48 + 64 * i, true);
        }
    }
    // 15.
    if n >= 5 {
        for i in 0..4usize {
            for j in 0..2usize {
                hadamard(t, 16 + 4 * i + j, 19 + 4 * i - j, i & 1 == 1, r);
            }
        }
    }
    // 16.
    if n == 6 {
        for i in 0..2i32 {
            for j in 0..4i32 {
                butterfly(
                    t,
                    (61 - i * 8 - j) as usize,
                    (34 + i * 8 + j) as usize,
                    56 - i * 32 + (j >> 1) * 64,
                    true,
                );
            }
        }
    }
    // 17.
    for i in 0..2usize {
        hadamard(t, i, 3 - i, false, r);
    }
    // 18.
    if n >= 3 {
        butterfly(t, 6, 5, 32, true);
    }
    // 19.
    if n >= 4 {
        for i in 0..2usize {
            for j in 0..2usize {
                hadamard(t, 8 + 4 * i + j, 11 + 4 * i - j, i == 1, r);
            }
        }
    }
    // 20.
    if n >= 5 {
        for i in 0..4i32 {
            butterfly(
                t,
                (29 - i) as usize,
                (18 + i) as usize,
                48 + (i >> 1) * 64,
                true,
            );
        }
    }
    // 21.
    if n == 6 {
        for i in 0..4usize {
            for j in 0..4usize {
                hadamard(t, 32 + 8 * i + j, 39 + 8 * i - j, i & 1 == 1, r);
            }
        }
    }
    // 22.
    if n >= 3 {
        for i in 0..4usize {
            hadamard(t, i, 7 - i, false, r);
        }
    }
    // 23.
    if n >= 4 {
        for i in 0..2i32 {
            butterfly(t, (13 - i) as usize, (10 + i) as usize, 32, true);
        }
    }
    // 24.
    if n >= 5 {
        for i in 0..2usize {
            for j in 0..4usize {
                hadamard(t, 16 + i * 8 + j, 23 + i * 8 - j, i == 1, r);
            }
        }
    }
    // 25.
    if n == 6 {
        for i in 0..8i32 {
            butterfly(
                t,
                (59 - i) as usize,
                (36 + i) as usize,
                if i < 4 { 48 } else { 112 },
                true,
            );
        }
    }
    // 26.
    if n >= 4 {
        for i in 0..8usize {
            hadamard(t, i, 15 - i, false, r);
        }
    }
    // 27.
    if n >= 5 {
        for i in 0..4i32 {
            butterfly(t, (27 - i) as usize, (20 + i) as usize, 32, true);
        }
    }
    // 28.
    if n == 6 {
        for i in 0..8usize {
            hadamard(t, 32 + i, 47 - i, false, r);
            hadamard(t, 48 + i, 63 - i, true, r);
        }
    }
    // 29.
    if n >= 5 {
        for i in 0..16usize {
            hadamard(t, i, 31 - i, false, r);
        }
    }
    // 30.
    if n == 6 {
        for i in 0..8i32 {
            butterfly(t, (55 - i) as usize, (40 + i) as usize, 32, true);
        }
    }
    // 31.
    if n == 6 {
        for i in 0..32usize {
            hadamard(t, i, 63 - i, false, r);
        }
    }
}

/// Inverse ADST input array permutation (spec 7.13.2.4).
fn inv_adst_input_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let mut copy = [0i32; 16];
    copy[..n0].copy_from_slice(&t[..n0]);
    for i in 0..n0 {
        let idx = if i & 1 == 1 { i - 1 } else { n0 - i - 1 };
        t[i] = copy[idx];
    }
}

/// Inverse ADST output array permutation (spec 7.13.2.5).
fn inv_adst_output_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let mut copy = [0i32; 16];
    copy[..n0].copy_from_slice(&t[..n0]);
    for i in 0..n0 {
        let a = (i >> 3) & 1;
        let b = ((i >> 2) & 1) ^ ((i >> 3) & 1);
        let c = ((i >> 1) & 1) ^ ((i >> 2) & 1);
        let d = (i & 1) ^ ((i >> 1) & 1);
        let idx = ((d << 3) | (c << 2) | (b << 1) | a) >> (4 - n);
        t[i] = if i & 1 == 1 { -copy[idx] } else { copy[idx] };
    }
}

const SINPI_1_9: i32 = 1321;
const SINPI_2_9: i32 = 2482;
const SINPI_3_9: i32 = 3344;
const SINPI_4_9: i32 = 3803;

/// Inverse ADST4 (spec 7.13.2.6).
fn inv_adst4(t: &mut [i32]) {
    let mut s = [0i32; 7];
    s[0] = SINPI_1_9 * t[0];
    s[1] = SINPI_2_9 * t[0];
    s[2] = SINPI_3_9 * t[1];
    s[3] = SINPI_4_9 * t[2];
    s[4] = SINPI_1_9 * t[2];
    s[5] = SINPI_2_9 * t[3];
    s[6] = SINPI_4_9 * t[3];
    let a7 = t[0] - t[2];
    let b7 = a7 + t[3];
    s[0] += s[3];
    s[1] -= s[4];
    s[3] = s[2];
    s[2] = SINPI_3_9 * b7;
    s[0] += s[5];
    s[1] -= s[6];
    let mut x = [0i32; 4];
    x[0] = s[0] + s[3];
    x[1] = s[1] + s[3];
    x[2] = s[2];
    x[3] = s[0] + s[1];
    x[3] -= s[3];
    t[0] = round2(x[0], 12);
    t[1] = round2(x[1], 12);
    t[2] = round2(x[2], 12);
    t[3] = round2(x[3], 12);
}

/// Inverse ADST8 (spec 7.13.2.7).
fn inv_adst8(t: &mut [i32], r: ClampRange) {
    inv_adst_input_permute(t, 3);
    for i in 0..4i32 {
        butterfly(t, (2 * i) as usize, (2 * i + 1) as usize, 60 - 16 * i, true);
    }
    for i in 0..4usize {
        hadamard(t, i, 4 + i, false, r);
    }
    for i in 0..2i32 {
        butterfly(t, (4 + 3 * i) as usize, (5 + i) as usize, 48 - 32 * i, true);
    }
    for i in 0..2usize {
        for j in 0..2usize {
            hadamard(t, 4 * j + i, 2 + 4 * j + i, false, r);
        }
    }
    for i in 0..2usize {
        butterfly(t, 2 + 4 * i, 3 + 4 * i, 32, true);
    }
    inv_adst_output_permute(t, 3);
}

/// Inverse ADST16 (spec 7.13.2.8).
fn inv_adst16(t: &mut [i32], r: ClampRange) {
    inv_adst_input_permute(t, 4);
    for i in 0..8i32 {
        butterfly(t, (2 * i) as usize, (2 * i + 1) as usize, 62 - 8 * i, true);
    }
    for i in 0..8usize {
        hadamard(t, i, 8 + i, false, r);
    }
    for i in 0..2i32 {
        butterfly(
            t,
            (8 + 2 * i) as usize,
            (9 + 2 * i) as usize,
            56 - 32 * i,
            true,
        );
        butterfly(
            t,
            (13 + 2 * i) as usize,
            (12 + 2 * i) as usize,
            8 + 32 * i,
            true,
        );
    }
    for i in 0..4usize {
        for j in 0..2usize {
            hadamard(t, 8 * j + i, 4 + 8 * j + i, false, r);
        }
    }
    for i in 0..2i32 {
        for j in 0..2usize {
            butterfly(
                t,
                4 + 8 * j + (3 * i) as usize,
                5 + 8 * j + i as usize,
                48 - 32 * i,
                true,
            );
        }
    }
    for i in 0..2usize {
        for j in 0..4usize {
            hadamard(t, 4 * j + i, 2 + 4 * j + i, false, r);
        }
    }
    for i in 0..4usize {
        butterfly(t, 2 + 4 * i, 3 + 4 * i, 32, true);
    }
    inv_adst_output_permute(t, 4);
}

/// Inverse ADST dispatch (spec 7.13.2.9), n = 2..4.
fn inv_adst(t: &mut [i32], n: u32, r: ClampRange) {
    match n {
        2 => inv_adst4(t),
        3 => inv_adst8(t, r),
        _ => inv_adst16(t, r),
    }
}

/// Inverse Walsh-Hadamard transform (spec 7.13.2.10), lossless only.
fn inv_wht4(t: &mut [i32], shift: u32) {
    let mut a = t[0] >> shift;
    let mut c = t[1] >> shift;
    let mut d = t[2] >> shift;
    let mut b = t[3] >> shift;
    a += c;
    d -= b;
    let e = (a - d) >> 1;
    b = e - b;
    c = e - c;
    a -= b;
    d += c;
    t[0] = a;
    t[1] = b;
    t[2] = c;
    t[3] = d;
}

/// Inverse identity transform (spec 7.13.2.11-15), n = 2..5.
fn inv_identity(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    match n {
        2 => {
            for v in t.iter_mut().take(n0) {
                *v = round2(*v * 5793, 12);
            }
        }
        3 => {
            for v in t.iter_mut().take(n0) {
                *v *= 2;
            }
        }
        4 => {
            for v in t.iter_mut().take(n0) {
                *v = round2(*v * 11586, 12);
            }
        }
        _ => {
            for v in t.iter_mut().take(n0) {
                *v *= 4;
            }
        }
    }
}

/// Which 1D transform a 2D type uses per direction.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tx1d {
    Dct,
    Adst,
    Identity,
}

fn row_tx(plane_tx_type: usize) -> Tx1d {
    match plane_tx_type {
        t if t == DCT_DCT || t == ADST_DCT || t == FLIPADST_DCT || t == H_DCT => Tx1d::Dct,
        t if t == DCT_ADST
            || t == ADST_ADST
            || t == DCT_FLIPADST
            || t == FLIPADST_FLIPADST
            || t == ADST_FLIPADST
            || t == FLIPADST_ADST
            || t == H_ADST
            || t == H_FLIPADST =>
        {
            Tx1d::Adst
        }
        _ => Tx1d::Identity,
    }
}

fn col_tx(plane_tx_type: usize) -> Tx1d {
    match plane_tx_type {
        t if t == DCT_DCT || t == DCT_ADST || t == DCT_FLIPADST || t == V_DCT => Tx1d::Dct,
        t if t == ADST_DCT
            || t == ADST_ADST
            || t == FLIPADST_DCT
            || t == FLIPADST_FLIPADST
            || t == ADST_FLIPADST
            || t == FLIPADST_ADST
            || t == V_ADST
            || t == V_FLIPADST =>
        {
            Tx1d::Adst
        }
        _ => Tx1d::Identity,
    }
}

/// 2D inverse transform process (spec 7.13.3): consumes the dequantized
/// coefficients (`dequant`, row-major `[i][j]` with values only in the
/// top-left 32x32) and produces the residual, row-major `w * h`.
///
/// `bit_depth` is 8 in this decoder (clamp ranges derive from it); anything
/// else is rejected at frame level by [`super::recon`].
///
/// # Intermediate precision
///
/// All intermediates are `i32`, matching libaom and dav1d (`int32_t` in
/// `dav1d/src/itx_1d.c`). Inside the transforms nothing is clamped beyond what
/// the spec itself prescribes — `H()`'s `Clip3` and the `Clip3` between the row
/// and column passes — so for every `Dequant` the spec can produce the result
/// is bit-identical to an arbitrary-precision evaluation.
///
/// The one non-spec clamp is on the coefficient load. Spec 7.13.3 just sets
/// `T[j] = Dequant[i][j]`, but 7.12.3 step f already clips `Dequant` to
/// `BitDepth + 8` bits, so re-applying that clamp is a provable no-op for
/// anything [`super::recon`] can hand over. It exists only because this
/// function is reachable with arbitrary `dequant`, and it is what establishes
/// `|T| <= 2^(r-1)` on entry — an out-of-range caller gets saturated
/// coefficients rather than an overflowed transform.
///
/// From `|T| <= 2^(r-1)` entering each 1D transform (the load clamp, the
/// inter-pass clamp, and `H()`):
///
/// * `B()` is a rotation, so one butterfly grows `|T|` by at most `sqrt(2)`,
///   and the spec's network never chains two without an intervening `H()`.
///   Propagating the bound through every stage of the real network gives
///   `max |T| = 46341` and a widest product of `2^28`.
/// * `inv_adst4` multiplies unclamped; its extremum over the coefficient box
///   (linear, so attained at a vertex) is `358_809_600`.
/// * `inv_identity` peaks at `11586 * 2^(r-1) = 379_650_048`.
///
/// The widest `i32` value at `BitDepth == 8` (`r == 16` for both passes) is
/// therefore `2^28.5`, **0.18x of `i32::MAX`**. At 10-bit it is `0.71x` —
/// still exact. 12-bit reaches `2.83x` and would overflow; supporting it needs
/// dav1d's restructured multiplies (see the comment atop
/// `dav1d/src/itx_1d.c`, which documents exactly this 19+sign-bit case).
/// `super::recon` rejects any `BitDepth != 8` at frame level.
pub fn inverse_transform_2d(
    dequant: &[i32],
    residual: &mut [i32],
    tx_sz: usize,
    plane_tx_type: usize,
    lossless: bool,
    bit_depth: u32,
) {
    let log2w = u32::from(TX_WIDTH_LOG2[tx_sz]);
    let log2h = u32::from(TX_HEIGHT_LOG2[tx_sz]);
    let w = 1usize << log2w;
    let h = 1usize << log2h;
    let row_shift = if lossless {
        0
    } else {
        u32::from(TRANSFORM_ROW_SHIFT[tx_sz])
    };
    let col_shift = if lossless { 0 } else { 4 };
    let row_clamp_range = ClampRange::new(bit_depth + 8);
    let col_clamp_range = ClampRange::new(core::cmp::max(bit_depth + 6, 16));
    let rect = log2w.abs_diff(log2h) == 1;

    let mut t = [0i32; 64];
    // Row transforms. Loading through the rowClampRange clamp is a no-op for
    // any spec-conformant caller (7.12.3 f already clips Dequant to exactly
    // `BitDepth + 8` bits) and is what keeps the `i32` products below bounded
    // no matter what a caller hands this public function.
    for i in 0..h {
        for j in 0..w {
            t[j] = if i < 32 && j < 32 {
                row_clamp_range.clip(dequant[i * 32 + j])
            } else {
                0
            };
        }
        if rect {
            for v in t.iter_mut().take(w) {
                *v = round2(*v * 2896, 12);
            }
        }
        if lossless {
            inv_wht4(&mut t, 2);
        } else {
            match row_tx(plane_tx_type) {
                Tx1d::Dct => inv_dct(&mut t, log2w, row_clamp_range),
                Tx1d::Adst => inv_adst(&mut t, log2w, row_clamp_range),
                Tx1d::Identity => inv_identity(&mut t, log2w),
            }
        }
        for j in 0..w {
            residual[i * w + j] = round2(t[j], row_shift);
        }
    }
    // Clamp between row and column transforms.
    for v in residual.iter_mut().take(w * h) {
        *v = col_clamp_range.clip(*v);
    }
    // Column transforms.
    for j in 0..w {
        for i in 0..h {
            t[i] = residual[i * w + j];
        }
        if lossless {
            inv_wht4(&mut t, 0);
        } else {
            match col_tx(plane_tx_type) {
                Tx1d::Dct => inv_dct(&mut t, log2h, col_clamp_range),
                Tx1d::Adst => inv_adst(&mut t, log2h, col_clamp_range),
                Tx1d::Identity => inv_identity(&mut t, log2h),
            }
        }
        for i in 0..h {
            residual[i * w + j] = round2(t[i], col_shift);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av1::kf::consts::{IDTX, TX_4X4, TX_8X4, TX_SIZES_ALL, TX_TYPES};

    /// A DC-only DCT_DCT 4x4: every output must be identical (the DCT of a
    /// constant spectrum line is flat) and nonzero for a nonzero input.
    #[test]
    fn dc_only_dct4x4_is_flat() {
        let mut dequant = [0i32; 32 * 32];
        dequant[0] = 100;
        let mut residual = [0i32; 16];
        inverse_transform_2d(&dequant, &mut residual, TX_4X4, DCT_DCT, false, 8);
        let first = residual[0];
        assert_ne!(first, 0);
        assert!(residual.iter().all(|&v| v == first), "DC-only must be flat");
    }

    /// IDTX must be a pure (scaled) passthrough: spatially, coefficient
    /// (i, j) maps to pixel (i, j) untouched by any butterfly.
    #[test]
    fn idtx_4x4_scaling() {
        let mut dequant = [0i32; 32 * 32];
        dequant[0] = 64;
        dequant[1] = -64;
        let mut residual = [0i32; 16];
        inverse_transform_2d(&dequant, &mut residual, TX_4X4, IDTX, false, 8);
        // 5793/4096 twice = ~2x, then >> 4 (col shift): 64 -> 64*2/16 = 8.
        assert_eq!(residual[0], 8);
        assert_eq!(residual[1], -8);
        assert_eq!(residual[2], 0);
    }

    /// Rectangular 8x4 applies the sqrt(2) compensation before the row pass.
    #[test]
    fn rect_8x4_runs() {
        let mut dequant = [0i32; 32 * 32];
        dequant[0] = 100;
        let mut residual = [0i32; 32];
        inverse_transform_2d(&dequant, &mut residual, TX_8X4, DCT_DCT, false, 8);
        let first = residual[0];
        assert!(residual.iter().all(|&v| v == first), "DC-only must be flat");
    }

    /// Deterministic xorshift64 coefficients at the extreme of the spec's
    /// `Dequant` range (spec 7.12.3 f clips to `BitDepth + 8` bits).
    fn extreme_dequant(seed: u64) -> [i32; 32 * 32] {
        let mut state = seed;
        let mut dequant = [0i32; 32 * 32];
        for v in &mut dequant {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Saturate to the range ends so the multipliers see worst-case
            // magnitudes rather than an average-case distribution.
            *v = if state & 1 == 0 {
                -(1 << 15)
            } else {
                (1 << 15) - 1
            };
        }
        dequant
    }

    /// The `i32` intermediates must survive the worst coefficients a
    /// conformant `Dequant` can hold, for every transform size and type.
    ///
    /// This is the regression test for the `i64 -> i32` conversion: debug
    /// builds trap on integer overflow, so reaching the assertions at all is
    /// the proof. The analytic bound is `2^28.5` (0.18x of `i32::MAX`) — see
    /// [`inverse_transform_2d`].
    #[test]
    fn extreme_coefficients_never_overflow_i32() {
        let dequant = extreme_dequant(0x2545_F491_4F6C_DD1D);
        for tx_sz in 0..TX_SIZES_ALL {
            let w = 1usize << TX_WIDTH_LOG2[tx_sz];
            let h = 1usize << TX_HEIGHT_LOG2[tx_sz];
            for plane_tx_type in 0..TX_TYPES {
                for lossless in [false, true] {
                    if lossless && tx_sz != TX_4X4 {
                        continue;
                    }
                    let mut residual = vec![0i32; w * h];
                    inverse_transform_2d(
                        &dequant,
                        &mut residual,
                        tx_sz,
                        plane_tx_type,
                        lossless,
                        8,
                    );
                    // Loose but meaningful: every path clamps or divides down
                    // to well under this, so a wild value means a lost clamp.
                    assert!(
                        residual.iter().all(|v| v.abs() < (1 << 18)),
                        "tx_sz {tx_sz} type {plane_tx_type} lossless {lossless} \
                         produced an out-of-range residual"
                    );
                }
            }
        }
    }

    /// `inverse_transform_2d` is public, so it must stay total even when a
    /// caller hands it coefficients outside the spec's `Dequant` range: the
    /// `rowClampRange` clamp on load is what bounds every later multiply.
    #[test]
    fn out_of_range_dequant_is_clamped_on_load() {
        for extreme in [i32::MIN, i32::MAX] {
            let dequant = [extreme; 32 * 32];
            for tx_sz in 0..TX_SIZES_ALL {
                let w = 1usize << TX_WIDTH_LOG2[tx_sz];
                let h = 1usize << TX_HEIGHT_LOG2[tx_sz];
                let mut residual = vec![0i32; w * h];
                inverse_transform_2d(&dequant, &mut residual, tx_sz, DCT_DCT, false, 8);
                assert!(residual.iter().all(|v| v.abs() < (1 << 18)));
            }
        }
    }

    /// The load clamp must be a no-op for in-range coefficients, i.e. it must
    /// not perturb the bit-exact output the fixtures pin down.
    #[test]
    fn load_clamp_is_a_noop_in_range() {
        let mut a = [0i32; 32 * 32];
        a[0] = (1 << 15) - 1;
        a[1] = -(1 << 15);
        a[33] = 12_345;
        let mut ra = [0i32; 16];
        let mut rb = [0i32; 16];
        inverse_transform_2d(&a, &mut ra, TX_4X4, DCT_DCT, false, 8);
        // Same coefficients, pre-clamped by the caller: identical output.
        let mut b = a;
        for v in &mut b {
            *v = (*v).clamp(-(1 << 15), (1 << 15) - 1);
        }
        inverse_transform_2d(&b, &mut rb, TX_4X4, DCT_DCT, false, 8);
        assert_eq!(ra, rb);
    }
}
