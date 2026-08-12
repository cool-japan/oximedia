//! VP9 inverse transforms — exact ports of libvpx `vpx_dsp/inv_txfm.c` and
//! the `vp9/common/vp9_idct.c` 2-D add wrappers (8-bit build).
//!
//! All lanes are `i64`; [`wraplow`] (truncate to `i16`, the sole rounding /
//! wrapping point of the 8-bit libvpx build) makes the arithmetic
//! value-identical to the reference `int16_t` step arrays: those arrays only
//! ever hold WRAPLOW'd or copied int16-range values.
//!
//! The eob-specialized libvpx variants (`idct8x8_12_add`, ...) are pure
//! speed optimizations that produce identical output; the full transforms
//! are used here unconditionally.

#![allow(clippy::needless_range_loop)]

const COSPI_1_64: i64 = 16364;
const COSPI_2_64: i64 = 16305;
const COSPI_3_64: i64 = 16207;
const COSPI_4_64: i64 = 16069;
const COSPI_5_64: i64 = 15893;
const COSPI_6_64: i64 = 15679;
const COSPI_7_64: i64 = 15426;
const COSPI_8_64: i64 = 15137;
const COSPI_9_64: i64 = 14811;
const COSPI_10_64: i64 = 14449;
const COSPI_11_64: i64 = 14053;
const COSPI_12_64: i64 = 13623;
const COSPI_13_64: i64 = 13160;
const COSPI_14_64: i64 = 12665;
const COSPI_15_64: i64 = 12140;
const COSPI_16_64: i64 = 11585;
const COSPI_17_64: i64 = 11003;
const COSPI_18_64: i64 = 10394;
const COSPI_19_64: i64 = 9760;
const COSPI_20_64: i64 = 9102;
const COSPI_21_64: i64 = 8423;
const COSPI_22_64: i64 = 7723;
const COSPI_23_64: i64 = 7005;
const COSPI_24_64: i64 = 6270;
const COSPI_25_64: i64 = 5520;
const COSPI_26_64: i64 = 4756;
const COSPI_27_64: i64 = 3981;
const COSPI_28_64: i64 = 3196;
const COSPI_29_64: i64 = 2404;
const COSPI_30_64: i64 = 1606;
const COSPI_31_64: i64 = 804;
const SINPI_1_9: i64 = 5283;
const SINPI_2_9: i64 = 9929;
const SINPI_3_9: i64 = 13377;
const SINPI_4_9: i64 = 15212;

/// libvpx `WRAPLOW` for the 8-bit build: truncate to `int16_t` range.
#[inline]
fn wraplow(x: i64) -> i64 {
    i64::from(x as i16)
}

/// libvpx `dct_const_round_shift` (round-shift by `DCT_CONST_BITS` = 14).
#[inline]
fn dct_round_shift(x: i64) -> i64 {
    (x + (1 << 13)) >> 14
}

/// libvpx `ROUND_POWER_OF_TWO`.
#[inline]
fn round_pow2(x: i64, n: u32) -> i64 {
    (x + (1i64 << (n - 1))) >> n
}

/// libvpx `clip_pixel_add`.
#[inline]
fn clip_pixel_add(dest: u8, trans: i64) -> u8 {
    (i64::from(dest) + trans).clamp(0, 255) as u8
}

/// Exact port of libvpx `idct4_c`.
fn idct4(input: &[i64], output: &mut [i64]) {
    let mut step = [0i64; 4];
    // stage 1
    let temp1 = (input[0] + input[2]) * COSPI_16_64;
    let temp2 = (input[0] - input[2]) * COSPI_16_64;
    step[0] = wraplow(dct_round_shift(temp1));
    step[1] = wraplow(dct_round_shift(temp2));
    let temp1 = input[1] * COSPI_24_64 - input[3] * COSPI_8_64;
    let temp2 = input[1] * COSPI_8_64 + input[3] * COSPI_24_64;
    step[2] = wraplow(dct_round_shift(temp1));
    step[3] = wraplow(dct_round_shift(temp2));
    // stage 2
    output[0] = wraplow(step[0] + step[3]);
    output[1] = wraplow(step[1] + step[2]);
    output[2] = wraplow(step[1] - step[2]);
    output[3] = wraplow(step[0] - step[3]);
}

/// Exact port of libvpx `iadst4_c`.
fn iadst4(input: &[i64], output: &mut [i64]) {
    let x0 = input[0];
    let x1 = input[1];
    let x2 = input[2];
    let x3 = input[3];
    if (x0 | x1 | x2 | x3) == 0 {
        output[..4].fill(0);
        return;
    }
    let mut s0 = SINPI_1_9 * x0;
    let mut s1 = SINPI_2_9 * x0;
    let mut s2 = SINPI_3_9 * x1;
    let mut s3 = SINPI_4_9 * x2;
    let s4 = SINPI_1_9 * x2;
    let s5 = SINPI_2_9 * x3;
    let s6 = SINPI_4_9 * x3;
    let s7 = wraplow(x0 - x2 + x3);

    s0 = s0 + s3 + s5;
    s1 = s1 - s4 - s6;
    s3 = s2;
    s2 = SINPI_3_9 * s7;

    output[0] = wraplow(dct_round_shift(s0 + s3));
    output[1] = wraplow(dct_round_shift(s1 + s3));
    output[2] = wraplow(dct_round_shift(s2));
    output[3] = wraplow(dct_round_shift(s0 + s1 - s3));
}

/// Exact port of libvpx `idct8_c`.
fn idct8(input: &[i64], output: &mut [i64]) {
    let mut step1 = [0i64; 8];
    let mut step2 = [0i64; 8];
    // stage 1
    step1[0] = input[0];
    step1[2] = input[4];
    step1[1] = input[2];
    step1[3] = input[6];
    let temp1 = input[1] * COSPI_28_64 - input[7] * COSPI_4_64;
    let temp2 = input[1] * COSPI_4_64 + input[7] * COSPI_28_64;
    step1[4] = wraplow(dct_round_shift(temp1));
    step1[7] = wraplow(dct_round_shift(temp2));
    let temp1 = input[5] * COSPI_12_64 - input[3] * COSPI_20_64;
    let temp2 = input[5] * COSPI_20_64 + input[3] * COSPI_12_64;
    step1[5] = wraplow(dct_round_shift(temp1));
    step1[6] = wraplow(dct_round_shift(temp2));

    // stage 2
    let temp1 = (step1[0] + step1[2]) * COSPI_16_64;
    let temp2 = (step1[0] - step1[2]) * COSPI_16_64;
    step2[0] = wraplow(dct_round_shift(temp1));
    step2[1] = wraplow(dct_round_shift(temp2));
    let temp1 = step1[1] * COSPI_24_64 - step1[3] * COSPI_8_64;
    let temp2 = step1[1] * COSPI_8_64 + step1[3] * COSPI_24_64;
    step2[2] = wraplow(dct_round_shift(temp1));
    step2[3] = wraplow(dct_round_shift(temp2));
    step2[4] = wraplow(step1[4] + step1[5]);
    step2[5] = wraplow(step1[4] - step1[5]);
    step2[6] = wraplow(-step1[6] + step1[7]);
    step2[7] = wraplow(step1[6] + step1[7]);

    // stage 3
    step1[0] = wraplow(step2[0] + step2[3]);
    step1[1] = wraplow(step2[1] + step2[2]);
    step1[2] = wraplow(step2[1] - step2[2]);
    step1[3] = wraplow(step2[0] - step2[3]);
    step1[4] = step2[4];
    let temp1 = (step2[6] - step2[5]) * COSPI_16_64;
    let temp2 = (step2[5] + step2[6]) * COSPI_16_64;
    step1[5] = wraplow(dct_round_shift(temp1));
    step1[6] = wraplow(dct_round_shift(temp2));
    step1[7] = step2[7];

    // stage 4
    output[0] = wraplow(step1[0] + step1[7]);
    output[1] = wraplow(step1[1] + step1[6]);
    output[2] = wraplow(step1[2] + step1[5]);
    output[3] = wraplow(step1[3] + step1[4]);
    output[4] = wraplow(step1[3] - step1[4]);
    output[5] = wraplow(step1[2] - step1[5]);
    output[6] = wraplow(step1[1] - step1[6]);
    output[7] = wraplow(step1[0] - step1[7]);
}

/// Exact port of libvpx `iadst8_c`.
fn iadst8(input: &[i64], output: &mut [i64]) {
    let mut x0 = input[7];
    let mut x1 = input[0];
    let mut x2 = input[5];
    let mut x3 = input[2];
    let mut x4 = input[3];
    let mut x5 = input[4];
    let mut x6 = input[1];
    let mut x7 = input[6];
    if (x0 | x1 | x2 | x3 | x4 | x5 | x6 | x7) == 0 {
        output[..8].fill(0);
        return;
    }

    // stage 1
    let mut s0 = COSPI_2_64 * x0 + COSPI_30_64 * x1;
    let mut s1 = COSPI_30_64 * x0 - COSPI_2_64 * x1;
    let mut s2 = COSPI_10_64 * x2 + COSPI_22_64 * x3;
    let mut s3 = COSPI_22_64 * x2 - COSPI_10_64 * x3;
    let mut s4 = COSPI_18_64 * x4 + COSPI_14_64 * x5;
    let mut s5 = COSPI_14_64 * x4 - COSPI_18_64 * x5;
    let mut s6 = COSPI_26_64 * x6 + COSPI_6_64 * x7;
    let mut s7 = COSPI_6_64 * x6 - COSPI_26_64 * x7;

    x0 = wraplow(dct_round_shift(s0 + s4));
    x1 = wraplow(dct_round_shift(s1 + s5));
    x2 = wraplow(dct_round_shift(s2 + s6));
    x3 = wraplow(dct_round_shift(s3 + s7));
    x4 = wraplow(dct_round_shift(s0 - s4));
    x5 = wraplow(dct_round_shift(s1 - s5));
    x6 = wraplow(dct_round_shift(s2 - s6));
    x7 = wraplow(dct_round_shift(s3 - s7));

    // stage 2
    s0 = x0;
    s1 = x1;
    s2 = x2;
    s3 = x3;
    s4 = COSPI_8_64 * x4 + COSPI_24_64 * x5;
    s5 = COSPI_24_64 * x4 - COSPI_8_64 * x5;
    s6 = -COSPI_24_64 * x6 + COSPI_8_64 * x7;
    s7 = COSPI_8_64 * x6 + COSPI_24_64 * x7;

    x0 = wraplow(s0 + s2);
    x1 = wraplow(s1 + s3);
    x2 = wraplow(s0 - s2);
    x3 = wraplow(s1 - s3);
    x4 = wraplow(dct_round_shift(s4 + s6));
    x5 = wraplow(dct_round_shift(s5 + s7));
    x6 = wraplow(dct_round_shift(s4 - s6));
    x7 = wraplow(dct_round_shift(s5 - s7));

    // stage 3
    s2 = COSPI_16_64 * (x2 + x3);
    s3 = COSPI_16_64 * (x2 - x3);
    s6 = COSPI_16_64 * (x6 + x7);
    s7 = COSPI_16_64 * (x6 - x7);

    x2 = wraplow(dct_round_shift(s2));
    x3 = wraplow(dct_round_shift(s3));
    x6 = wraplow(dct_round_shift(s6));
    x7 = wraplow(dct_round_shift(s7));

    output[0] = wraplow(x0);
    output[1] = wraplow(-x4);
    output[2] = wraplow(x6);
    output[3] = wraplow(-x2);
    output[4] = wraplow(x3);
    output[5] = wraplow(-x7);
    output[6] = wraplow(x5);
    output[7] = wraplow(-x1);
}
/// Exact port of libvpx `idct16_c` (vpx_dsp/inv_txfm.c), 8-bit build.
#[allow(clippy::too_many_lines)]
fn idct16(input: &[i64], output: &mut [i64]) {
    let mut step1 = [0i64; 16];
    let mut step2 = [0i64; 16];
    let mut temp1: i64;
    let mut temp2: i64;
    // stage 1
    step1[0] = input[0];
    step1[1] = input[8];
    step1[2] = input[4];
    step1[3] = input[12];
    step1[4] = input[2];
    step1[5] = input[10];
    step1[6] = input[6];
    step1[7] = input[14];
    step1[8] = input[1];
    step1[9] = input[9];
    step1[10] = input[5];
    step1[11] = input[13];
    step1[12] = input[3];
    step1[13] = input[11];
    step1[14] = input[7];
    step1[15] = input[15];
    // stage 2
    step2[0] = step1[0];
    step2[1] = step1[1];
    step2[2] = step1[2];
    step2[3] = step1[3];
    step2[4] = step1[4];
    step2[5] = step1[5];
    step2[6] = step1[6];
    step2[7] = step1[7];
    temp1 = step1[8] * COSPI_30_64 - step1[15] * COSPI_2_64;
    temp2 = step1[8] * COSPI_2_64 + step1[15] * COSPI_30_64;
    step2[8] = wraplow(dct_round_shift(temp1));
    step2[15] = wraplow(dct_round_shift(temp2));
    temp1 = step1[9] * COSPI_14_64 - step1[14] * COSPI_18_64;
    temp2 = step1[9] * COSPI_18_64 + step1[14] * COSPI_14_64;
    step2[9] = wraplow(dct_round_shift(temp1));
    step2[14] = wraplow(dct_round_shift(temp2));
    temp1 = step1[10] * COSPI_22_64 - step1[13] * COSPI_10_64;
    temp2 = step1[10] * COSPI_10_64 + step1[13] * COSPI_22_64;
    step2[10] = wraplow(dct_round_shift(temp1));
    step2[13] = wraplow(dct_round_shift(temp2));
    temp1 = step1[11] * COSPI_6_64 - step1[12] * COSPI_26_64;
    temp2 = step1[11] * COSPI_26_64 + step1[12] * COSPI_6_64;
    step2[11] = wraplow(dct_round_shift(temp1));
    step2[12] = wraplow(dct_round_shift(temp2));
    // stage 3
    step1[0] = step2[0];
    step1[1] = step2[1];
    step1[2] = step2[2];
    step1[3] = step2[3];
    temp1 = step2[4] * COSPI_28_64 - step2[7] * COSPI_4_64;
    temp2 = step2[4] * COSPI_4_64 + step2[7] * COSPI_28_64;
    step1[4] = wraplow(dct_round_shift(temp1));
    step1[7] = wraplow(dct_round_shift(temp2));
    temp1 = step2[5] * COSPI_12_64 - step2[6] * COSPI_20_64;
    temp2 = step2[5] * COSPI_20_64 + step2[6] * COSPI_12_64;
    step1[5] = wraplow(dct_round_shift(temp1));
    step1[6] = wraplow(dct_round_shift(temp2));
    step1[8] = wraplow(step2[8] + step2[9]);
    step1[9] = wraplow(step2[8] - step2[9]);
    step1[10] = wraplow(-step2[10] + step2[11]);
    step1[11] = wraplow(step2[10] + step2[11]);
    step1[12] = wraplow(step2[12] + step2[13]);
    step1[13] = wraplow(step2[12] - step2[13]);
    step1[14] = wraplow(-step2[14] + step2[15]);
    step1[15] = wraplow(step2[14] + step2[15]);
    // stage 4
    temp1 = (step1[0] + step1[1]) * COSPI_16_64;
    temp2 = (step1[0] - step1[1]) * COSPI_16_64;
    step2[0] = wraplow(dct_round_shift(temp1));
    step2[1] = wraplow(dct_round_shift(temp2));
    temp1 = step1[2] * COSPI_24_64 - step1[3] * COSPI_8_64;
    temp2 = step1[2] * COSPI_8_64 + step1[3] * COSPI_24_64;
    step2[2] = wraplow(dct_round_shift(temp1));
    step2[3] = wraplow(dct_round_shift(temp2));
    step2[4] = wraplow(step1[4] + step1[5]);
    step2[5] = wraplow(step1[4] - step1[5]);
    step2[6] = wraplow(-step1[6] + step1[7]);
    step2[7] = wraplow(step1[6] + step1[7]);
    step2[8] = step1[8];
    step2[15] = step1[15];
    temp1 = -step1[9] * COSPI_8_64 + step1[14] * COSPI_24_64;
    temp2 = step1[9] * COSPI_24_64 + step1[14] * COSPI_8_64;
    step2[9] = wraplow(dct_round_shift(temp1));
    step2[14] = wraplow(dct_round_shift(temp2));
    temp1 = -step1[10] * COSPI_24_64 - step1[13] * COSPI_8_64;
    temp2 = -step1[10] * COSPI_8_64 + step1[13] * COSPI_24_64;
    step2[10] = wraplow(dct_round_shift(temp1));
    step2[13] = wraplow(dct_round_shift(temp2));
    step2[11] = step1[11];
    step2[12] = step1[12];
    // stage 5
    step1[0] = wraplow(step2[0] + step2[3]);
    step1[1] = wraplow(step2[1] + step2[2]);
    step1[2] = wraplow(step2[1] - step2[2]);
    step1[3] = wraplow(step2[0] - step2[3]);
    step1[4] = step2[4];
    temp1 = (step2[6] - step2[5]) * COSPI_16_64;
    temp2 = (step2[5] + step2[6]) * COSPI_16_64;
    step1[5] = wraplow(dct_round_shift(temp1));
    step1[6] = wraplow(dct_round_shift(temp2));
    step1[7] = step2[7];
    step1[8] = wraplow(step2[8] + step2[11]);
    step1[9] = wraplow(step2[9] + step2[10]);
    step1[10] = wraplow(step2[9] - step2[10]);
    step1[11] = wraplow(step2[8] - step2[11]);
    step1[12] = wraplow(-step2[12] + step2[15]);
    step1[13] = wraplow(-step2[13] + step2[14]);
    step1[14] = wraplow(step2[13] + step2[14]);
    step1[15] = wraplow(step2[12] + step2[15]);
    // stage 6
    step2[0] = wraplow(step1[0] + step1[7]);
    step2[1] = wraplow(step1[1] + step1[6]);
    step2[2] = wraplow(step1[2] + step1[5]);
    step2[3] = wraplow(step1[3] + step1[4]);
    step2[4] = wraplow(step1[3] - step1[4]);
    step2[5] = wraplow(step1[2] - step1[5]);
    step2[6] = wraplow(step1[1] - step1[6]);
    step2[7] = wraplow(step1[0] - step1[7]);
    step2[8] = step1[8];
    step2[9] = step1[9];
    temp1 = (-step1[10] + step1[13]) * COSPI_16_64;
    temp2 = (step1[10] + step1[13]) * COSPI_16_64;
    step2[10] = wraplow(dct_round_shift(temp1));
    step2[13] = wraplow(dct_round_shift(temp2));
    temp1 = (-step1[11] + step1[12]) * COSPI_16_64;
    temp2 = (step1[11] + step1[12]) * COSPI_16_64;
    step2[11] = wraplow(dct_round_shift(temp1));
    step2[12] = wraplow(dct_round_shift(temp2));
    step2[14] = step1[14];
    step2[15] = step1[15];
    // stage 7
    output[0] = wraplow(step2[0] + step2[15]);
    output[1] = wraplow(step2[1] + step2[14]);
    output[2] = wraplow(step2[2] + step2[13]);
    output[3] = wraplow(step2[3] + step2[12]);
    output[4] = wraplow(step2[4] + step2[11]);
    output[5] = wraplow(step2[5] + step2[10]);
    output[6] = wraplow(step2[6] + step2[9]);
    output[7] = wraplow(step2[7] + step2[8]);
    output[8] = wraplow(step2[7] - step2[8]);
    output[9] = wraplow(step2[6] - step2[9]);
    output[10] = wraplow(step2[5] - step2[10]);
    output[11] = wraplow(step2[4] - step2[11]);
    output[12] = wraplow(step2[3] - step2[12]);
    output[13] = wraplow(step2[2] - step2[13]);
    output[14] = wraplow(step2[1] - step2[14]);
    output[15] = wraplow(step2[0] - step2[15]);
}

/// Exact port of libvpx `iadst16_c` (vpx_dsp/inv_txfm.c), 8-bit build.
#[allow(clippy::too_many_lines)]
fn iadst16(input: &[i64], output: &mut [i64]) {
    let mut s0: i64;
    let mut s1: i64;
    let mut s2: i64;
    let mut s3: i64;
    let mut s4: i64;
    let mut s5: i64;
    let mut s6: i64;
    let mut s7: i64;
    let mut s8: i64;
    let mut s9: i64;
    let mut s10: i64;
    let mut s11: i64;
    let mut s12: i64;
    let mut s13: i64;
    let mut s14: i64;
    let mut s15: i64;
    let mut x0: i64 = input[15];
    let mut x1: i64 = input[0];
    let mut x2: i64 = input[13];
    let mut x3: i64 = input[2];
    let mut x4: i64 = input[11];
    let mut x5: i64 = input[4];
    let mut x6: i64 = input[9];
    let mut x7: i64 = input[6];
    let mut x8: i64 = input[7];
    let mut x9: i64 = input[8];
    let mut x10: i64 = input[5];
    let mut x11: i64 = input[10];
    let mut x12: i64 = input[3];
    let mut x13: i64 = input[12];
    let mut x14: i64 = input[1];
    let mut x15: i64 = input[14];
    if (x0 | x1 | x2 | x3 | x4 | x5 | x6 | x7 | x8 | x9 | x10 | x11 | x12 | x13 | x14 | x15) == 0 {
        output[..16].fill(0);
        return;
    }
    // stage 1
    s0 = x0 * COSPI_1_64 + x1 * COSPI_31_64;
    s1 = x0 * COSPI_31_64 - x1 * COSPI_1_64;
    s2 = x2 * COSPI_5_64 + x3 * COSPI_27_64;
    s3 = x2 * COSPI_27_64 - x3 * COSPI_5_64;
    s4 = x4 * COSPI_9_64 + x5 * COSPI_23_64;
    s5 = x4 * COSPI_23_64 - x5 * COSPI_9_64;
    s6 = x6 * COSPI_13_64 + x7 * COSPI_19_64;
    s7 = x6 * COSPI_19_64 - x7 * COSPI_13_64;
    s8 = x8 * COSPI_17_64 + x9 * COSPI_15_64;
    s9 = x8 * COSPI_15_64 - x9 * COSPI_17_64;
    s10 = x10 * COSPI_21_64 + x11 * COSPI_11_64;
    s11 = x10 * COSPI_11_64 - x11 * COSPI_21_64;
    s12 = x12 * COSPI_25_64 + x13 * COSPI_7_64;
    s13 = x12 * COSPI_7_64 - x13 * COSPI_25_64;
    s14 = x14 * COSPI_29_64 + x15 * COSPI_3_64;
    s15 = x14 * COSPI_3_64 - x15 * COSPI_29_64;
    x0 = wraplow(dct_round_shift(s0 + s8));
    x1 = wraplow(dct_round_shift(s1 + s9));
    x2 = wraplow(dct_round_shift(s2 + s10));
    x3 = wraplow(dct_round_shift(s3 + s11));
    x4 = wraplow(dct_round_shift(s4 + s12));
    x5 = wraplow(dct_round_shift(s5 + s13));
    x6 = wraplow(dct_round_shift(s6 + s14));
    x7 = wraplow(dct_round_shift(s7 + s15));
    x8 = wraplow(dct_round_shift(s0 - s8));
    x9 = wraplow(dct_round_shift(s1 - s9));
    x10 = wraplow(dct_round_shift(s2 - s10));
    x11 = wraplow(dct_round_shift(s3 - s11));
    x12 = wraplow(dct_round_shift(s4 - s12));
    x13 = wraplow(dct_round_shift(s5 - s13));
    x14 = wraplow(dct_round_shift(s6 - s14));
    x15 = wraplow(dct_round_shift(s7 - s15));
    // stage 2
    s0 = x0;
    s1 = x1;
    s2 = x2;
    s3 = x3;
    s4 = x4;
    s5 = x5;
    s6 = x6;
    s7 = x7;
    s8 = x8 * COSPI_4_64 + x9 * COSPI_28_64;
    s9 = x8 * COSPI_28_64 - x9 * COSPI_4_64;
    s10 = x10 * COSPI_20_64 + x11 * COSPI_12_64;
    s11 = x10 * COSPI_12_64 - x11 * COSPI_20_64;
    s12 = -x12 * COSPI_28_64 + x13 * COSPI_4_64;
    s13 = x12 * COSPI_4_64 + x13 * COSPI_28_64;
    s14 = -x14 * COSPI_12_64 + x15 * COSPI_20_64;
    s15 = x14 * COSPI_20_64 + x15 * COSPI_12_64;
    x0 = wraplow(s0 + s4);
    x1 = wraplow(s1 + s5);
    x2 = wraplow(s2 + s6);
    x3 = wraplow(s3 + s7);
    x4 = wraplow(s0 - s4);
    x5 = wraplow(s1 - s5);
    x6 = wraplow(s2 - s6);
    x7 = wraplow(s3 - s7);
    x8 = wraplow(dct_round_shift(s8 + s12));
    x9 = wraplow(dct_round_shift(s9 + s13));
    x10 = wraplow(dct_round_shift(s10 + s14));
    x11 = wraplow(dct_round_shift(s11 + s15));
    x12 = wraplow(dct_round_shift(s8 - s12));
    x13 = wraplow(dct_round_shift(s9 - s13));
    x14 = wraplow(dct_round_shift(s10 - s14));
    x15 = wraplow(dct_round_shift(s11 - s15));
    // stage 3
    s0 = x0;
    s1 = x1;
    s2 = x2;
    s3 = x3;
    s4 = x4 * COSPI_8_64 + x5 * COSPI_24_64;
    s5 = x4 * COSPI_24_64 - x5 * COSPI_8_64;
    s6 = -x6 * COSPI_24_64 + x7 * COSPI_8_64;
    s7 = x6 * COSPI_8_64 + x7 * COSPI_24_64;
    s8 = x8;
    s9 = x9;
    s10 = x10;
    s11 = x11;
    s12 = x12 * COSPI_8_64 + x13 * COSPI_24_64;
    s13 = x12 * COSPI_24_64 - x13 * COSPI_8_64;
    s14 = -x14 * COSPI_24_64 + x15 * COSPI_8_64;
    s15 = x14 * COSPI_8_64 + x15 * COSPI_24_64;
    x0 = wraplow(s0 + s2);
    x1 = wraplow(s1 + s3);
    x2 = wraplow(s0 - s2);
    x3 = wraplow(s1 - s3);
    x4 = wraplow(dct_round_shift(s4 + s6));
    x5 = wraplow(dct_round_shift(s5 + s7));
    x6 = wraplow(dct_round_shift(s4 - s6));
    x7 = wraplow(dct_round_shift(s5 - s7));
    x8 = wraplow(s8 + s10);
    x9 = wraplow(s9 + s11);
    x10 = wraplow(s8 - s10);
    x11 = wraplow(s9 - s11);
    x12 = wraplow(dct_round_shift(s12 + s14));
    x13 = wraplow(dct_round_shift(s13 + s15));
    x14 = wraplow(dct_round_shift(s12 - s14));
    x15 = wraplow(dct_round_shift(s13 - s15));
    // stage 4
    s2 = (-COSPI_16_64) * (x2 + x3);
    s3 = COSPI_16_64 * (x2 - x3);
    s6 = COSPI_16_64 * (x6 + x7);
    s7 = COSPI_16_64 * (-x6 + x7);
    s10 = COSPI_16_64 * (x10 + x11);
    s11 = COSPI_16_64 * (-x10 + x11);
    s14 = (-COSPI_16_64) * (x14 + x15);
    s15 = COSPI_16_64 * (x14 - x15);
    x2 = wraplow(dct_round_shift(s2));
    x3 = wraplow(dct_round_shift(s3));
    x6 = wraplow(dct_round_shift(s6));
    x7 = wraplow(dct_round_shift(s7));
    x10 = wraplow(dct_round_shift(s10));
    x11 = wraplow(dct_round_shift(s11));
    x14 = wraplow(dct_round_shift(s14));
    x15 = wraplow(dct_round_shift(s15));
    output[0] = wraplow(x0);
    output[1] = wraplow(-x8);
    output[2] = wraplow(x12);
    output[3] = wraplow(-x4);
    output[4] = wraplow(x6);
    output[5] = wraplow(x14);
    output[6] = wraplow(x10);
    output[7] = wraplow(x2);
    output[8] = wraplow(x3);
    output[9] = wraplow(x11);
    output[10] = wraplow(x15);
    output[11] = wraplow(x7);
    output[12] = wraplow(x5);
    output[13] = wraplow(-x13);
    output[14] = wraplow(x9);
    output[15] = wraplow(-x1);
}

/// Exact port of libvpx `idct32_c` (vpx_dsp/inv_txfm.c), 8-bit build.
#[allow(clippy::too_many_lines)]
fn idct32(input: &[i64], output: &mut [i64]) {
    let mut step1 = [0i64; 32];
    let mut step2 = [0i64; 32];
    let mut temp1: i64;
    let mut temp2: i64;
    // stage 1
    step1[0] = input[0];
    step1[1] = input[16];
    step1[2] = input[8];
    step1[3] = input[24];
    step1[4] = input[4];
    step1[5] = input[20];
    step1[6] = input[12];
    step1[7] = input[28];
    step1[8] = input[2];
    step1[9] = input[18];
    step1[10] = input[10];
    step1[11] = input[26];
    step1[12] = input[6];
    step1[13] = input[22];
    step1[14] = input[14];
    step1[15] = input[30];
    temp1 = input[1] * COSPI_31_64 - input[31] * COSPI_1_64;
    temp2 = input[1] * COSPI_1_64 + input[31] * COSPI_31_64;
    step1[16] = wraplow(dct_round_shift(temp1));
    step1[31] = wraplow(dct_round_shift(temp2));
    temp1 = input[17] * COSPI_15_64 - input[15] * COSPI_17_64;
    temp2 = input[17] * COSPI_17_64 + input[15] * COSPI_15_64;
    step1[17] = wraplow(dct_round_shift(temp1));
    step1[30] = wraplow(dct_round_shift(temp2));
    temp1 = input[9] * COSPI_23_64 - input[23] * COSPI_9_64;
    temp2 = input[9] * COSPI_9_64 + input[23] * COSPI_23_64;
    step1[18] = wraplow(dct_round_shift(temp1));
    step1[29] = wraplow(dct_round_shift(temp2));
    temp1 = input[25] * COSPI_7_64 - input[7] * COSPI_25_64;
    temp2 = input[25] * COSPI_25_64 + input[7] * COSPI_7_64;
    step1[19] = wraplow(dct_round_shift(temp1));
    step1[28] = wraplow(dct_round_shift(temp2));
    temp1 = input[5] * COSPI_27_64 - input[27] * COSPI_5_64;
    temp2 = input[5] * COSPI_5_64 + input[27] * COSPI_27_64;
    step1[20] = wraplow(dct_round_shift(temp1));
    step1[27] = wraplow(dct_round_shift(temp2));
    temp1 = input[21] * COSPI_11_64 - input[11] * COSPI_21_64;
    temp2 = input[21] * COSPI_21_64 + input[11] * COSPI_11_64;
    step1[21] = wraplow(dct_round_shift(temp1));
    step1[26] = wraplow(dct_round_shift(temp2));
    temp1 = input[13] * COSPI_19_64 - input[19] * COSPI_13_64;
    temp2 = input[13] * COSPI_13_64 + input[19] * COSPI_19_64;
    step1[22] = wraplow(dct_round_shift(temp1));
    step1[25] = wraplow(dct_round_shift(temp2));
    temp1 = input[29] * COSPI_3_64 - input[3] * COSPI_29_64;
    temp2 = input[29] * COSPI_29_64 + input[3] * COSPI_3_64;
    step1[23] = wraplow(dct_round_shift(temp1));
    step1[24] = wraplow(dct_round_shift(temp2));
    // stage 2
    step2[0] = step1[0];
    step2[1] = step1[1];
    step2[2] = step1[2];
    step2[3] = step1[3];
    step2[4] = step1[4];
    step2[5] = step1[5];
    step2[6] = step1[6];
    step2[7] = step1[7];
    temp1 = step1[8] * COSPI_30_64 - step1[15] * COSPI_2_64;
    temp2 = step1[8] * COSPI_2_64 + step1[15] * COSPI_30_64;
    step2[8] = wraplow(dct_round_shift(temp1));
    step2[15] = wraplow(dct_round_shift(temp2));
    temp1 = step1[9] * COSPI_14_64 - step1[14] * COSPI_18_64;
    temp2 = step1[9] * COSPI_18_64 + step1[14] * COSPI_14_64;
    step2[9] = wraplow(dct_round_shift(temp1));
    step2[14] = wraplow(dct_round_shift(temp2));
    temp1 = step1[10] * COSPI_22_64 - step1[13] * COSPI_10_64;
    temp2 = step1[10] * COSPI_10_64 + step1[13] * COSPI_22_64;
    step2[10] = wraplow(dct_round_shift(temp1));
    step2[13] = wraplow(dct_round_shift(temp2));
    temp1 = step1[11] * COSPI_6_64 - step1[12] * COSPI_26_64;
    temp2 = step1[11] * COSPI_26_64 + step1[12] * COSPI_6_64;
    step2[11] = wraplow(dct_round_shift(temp1));
    step2[12] = wraplow(dct_round_shift(temp2));
    step2[16] = wraplow(step1[16] + step1[17]);
    step2[17] = wraplow(step1[16] - step1[17]);
    step2[18] = wraplow(-step1[18] + step1[19]);
    step2[19] = wraplow(step1[18] + step1[19]);
    step2[20] = wraplow(step1[20] + step1[21]);
    step2[21] = wraplow(step1[20] - step1[21]);
    step2[22] = wraplow(-step1[22] + step1[23]);
    step2[23] = wraplow(step1[22] + step1[23]);
    step2[24] = wraplow(step1[24] + step1[25]);
    step2[25] = wraplow(step1[24] - step1[25]);
    step2[26] = wraplow(-step1[26] + step1[27]);
    step2[27] = wraplow(step1[26] + step1[27]);
    step2[28] = wraplow(step1[28] + step1[29]);
    step2[29] = wraplow(step1[28] - step1[29]);
    step2[30] = wraplow(-step1[30] + step1[31]);
    step2[31] = wraplow(step1[30] + step1[31]);
    // stage 3
    step1[0] = step2[0];
    step1[1] = step2[1];
    step1[2] = step2[2];
    step1[3] = step2[3];
    temp1 = step2[4] * COSPI_28_64 - step2[7] * COSPI_4_64;
    temp2 = step2[4] * COSPI_4_64 + step2[7] * COSPI_28_64;
    step1[4] = wraplow(dct_round_shift(temp1));
    step1[7] = wraplow(dct_round_shift(temp2));
    temp1 = step2[5] * COSPI_12_64 - step2[6] * COSPI_20_64;
    temp2 = step2[5] * COSPI_20_64 + step2[6] * COSPI_12_64;
    step1[5] = wraplow(dct_round_shift(temp1));
    step1[6] = wraplow(dct_round_shift(temp2));
    step1[8] = wraplow(step2[8] + step2[9]);
    step1[9] = wraplow(step2[8] - step2[9]);
    step1[10] = wraplow(-step2[10] + step2[11]);
    step1[11] = wraplow(step2[10] + step2[11]);
    step1[12] = wraplow(step2[12] + step2[13]);
    step1[13] = wraplow(step2[12] - step2[13]);
    step1[14] = wraplow(-step2[14] + step2[15]);
    step1[15] = wraplow(step2[14] + step2[15]);
    step1[16] = step2[16];
    step1[31] = step2[31];
    temp1 = -step2[17] * COSPI_4_64 + step2[30] * COSPI_28_64;
    temp2 = step2[17] * COSPI_28_64 + step2[30] * COSPI_4_64;
    step1[17] = wraplow(dct_round_shift(temp1));
    step1[30] = wraplow(dct_round_shift(temp2));
    temp1 = -step2[18] * COSPI_28_64 - step2[29] * COSPI_4_64;
    temp2 = -step2[18] * COSPI_4_64 + step2[29] * COSPI_28_64;
    step1[18] = wraplow(dct_round_shift(temp1));
    step1[29] = wraplow(dct_round_shift(temp2));
    step1[19] = step2[19];
    step1[20] = step2[20];
    temp1 = -step2[21] * COSPI_20_64 + step2[26] * COSPI_12_64;
    temp2 = step2[21] * COSPI_12_64 + step2[26] * COSPI_20_64;
    step1[21] = wraplow(dct_round_shift(temp1));
    step1[26] = wraplow(dct_round_shift(temp2));
    temp1 = -step2[22] * COSPI_12_64 - step2[25] * COSPI_20_64;
    temp2 = -step2[22] * COSPI_20_64 + step2[25] * COSPI_12_64;
    step1[22] = wraplow(dct_round_shift(temp1));
    step1[25] = wraplow(dct_round_shift(temp2));
    step1[23] = step2[23];
    step1[24] = step2[24];
    step1[27] = step2[27];
    step1[28] = step2[28];
    // stage 4
    temp1 = (step1[0] + step1[1]) * COSPI_16_64;
    temp2 = (step1[0] - step1[1]) * COSPI_16_64;
    step2[0] = wraplow(dct_round_shift(temp1));
    step2[1] = wraplow(dct_round_shift(temp2));
    temp1 = step1[2] * COSPI_24_64 - step1[3] * COSPI_8_64;
    temp2 = step1[2] * COSPI_8_64 + step1[3] * COSPI_24_64;
    step2[2] = wraplow(dct_round_shift(temp1));
    step2[3] = wraplow(dct_round_shift(temp2));
    step2[4] = wraplow(step1[4] + step1[5]);
    step2[5] = wraplow(step1[4] - step1[5]);
    step2[6] = wraplow(-step1[6] + step1[7]);
    step2[7] = wraplow(step1[6] + step1[7]);
    step2[8] = step1[8];
    step2[15] = step1[15];
    temp1 = -step1[9] * COSPI_8_64 + step1[14] * COSPI_24_64;
    temp2 = step1[9] * COSPI_24_64 + step1[14] * COSPI_8_64;
    step2[9] = wraplow(dct_round_shift(temp1));
    step2[14] = wraplow(dct_round_shift(temp2));
    temp1 = -step1[10] * COSPI_24_64 - step1[13] * COSPI_8_64;
    temp2 = -step1[10] * COSPI_8_64 + step1[13] * COSPI_24_64;
    step2[10] = wraplow(dct_round_shift(temp1));
    step2[13] = wraplow(dct_round_shift(temp2));
    step2[11] = step1[11];
    step2[12] = step1[12];
    step2[16] = wraplow(step1[16] + step1[19]);
    step2[17] = wraplow(step1[17] + step1[18]);
    step2[18] = wraplow(step1[17] - step1[18]);
    step2[19] = wraplow(step1[16] - step1[19]);
    step2[20] = wraplow(-step1[20] + step1[23]);
    step2[21] = wraplow(-step1[21] + step1[22]);
    step2[22] = wraplow(step1[21] + step1[22]);
    step2[23] = wraplow(step1[20] + step1[23]);
    step2[24] = wraplow(step1[24] + step1[27]);
    step2[25] = wraplow(step1[25] + step1[26]);
    step2[26] = wraplow(step1[25] - step1[26]);
    step2[27] = wraplow(step1[24] - step1[27]);
    step2[28] = wraplow(-step1[28] + step1[31]);
    step2[29] = wraplow(-step1[29] + step1[30]);
    step2[30] = wraplow(step1[29] + step1[30]);
    step2[31] = wraplow(step1[28] + step1[31]);
    // stage 5
    step1[0] = wraplow(step2[0] + step2[3]);
    step1[1] = wraplow(step2[1] + step2[2]);
    step1[2] = wraplow(step2[1] - step2[2]);
    step1[3] = wraplow(step2[0] - step2[3]);
    step1[4] = step2[4];
    temp1 = (step2[6] - step2[5]) * COSPI_16_64;
    temp2 = (step2[5] + step2[6]) * COSPI_16_64;
    step1[5] = wraplow(dct_round_shift(temp1));
    step1[6] = wraplow(dct_round_shift(temp2));
    step1[7] = step2[7];
    step1[8] = wraplow(step2[8] + step2[11]);
    step1[9] = wraplow(step2[9] + step2[10]);
    step1[10] = wraplow(step2[9] - step2[10]);
    step1[11] = wraplow(step2[8] - step2[11]);
    step1[12] = wraplow(-step2[12] + step2[15]);
    step1[13] = wraplow(-step2[13] + step2[14]);
    step1[14] = wraplow(step2[13] + step2[14]);
    step1[15] = wraplow(step2[12] + step2[15]);
    step1[16] = step2[16];
    step1[17] = step2[17];
    temp1 = -step2[18] * COSPI_8_64 + step2[29] * COSPI_24_64;
    temp2 = step2[18] * COSPI_24_64 + step2[29] * COSPI_8_64;
    step1[18] = wraplow(dct_round_shift(temp1));
    step1[29] = wraplow(dct_round_shift(temp2));
    temp1 = -step2[19] * COSPI_8_64 + step2[28] * COSPI_24_64;
    temp2 = step2[19] * COSPI_24_64 + step2[28] * COSPI_8_64;
    step1[19] = wraplow(dct_round_shift(temp1));
    step1[28] = wraplow(dct_round_shift(temp2));
    temp1 = -step2[20] * COSPI_24_64 - step2[27] * COSPI_8_64;
    temp2 = -step2[20] * COSPI_8_64 + step2[27] * COSPI_24_64;
    step1[20] = wraplow(dct_round_shift(temp1));
    step1[27] = wraplow(dct_round_shift(temp2));
    temp1 = -step2[21] * COSPI_24_64 - step2[26] * COSPI_8_64;
    temp2 = -step2[21] * COSPI_8_64 + step2[26] * COSPI_24_64;
    step1[21] = wraplow(dct_round_shift(temp1));
    step1[26] = wraplow(dct_round_shift(temp2));
    step1[22] = step2[22];
    step1[23] = step2[23];
    step1[24] = step2[24];
    step1[25] = step2[25];
    step1[30] = step2[30];
    step1[31] = step2[31];
    // stage 6
    step2[0] = wraplow(step1[0] + step1[7]);
    step2[1] = wraplow(step1[1] + step1[6]);
    step2[2] = wraplow(step1[2] + step1[5]);
    step2[3] = wraplow(step1[3] + step1[4]);
    step2[4] = wraplow(step1[3] - step1[4]);
    step2[5] = wraplow(step1[2] - step1[5]);
    step2[6] = wraplow(step1[1] - step1[6]);
    step2[7] = wraplow(step1[0] - step1[7]);
    step2[8] = step1[8];
    step2[9] = step1[9];
    temp1 = (-step1[10] + step1[13]) * COSPI_16_64;
    temp2 = (step1[10] + step1[13]) * COSPI_16_64;
    step2[10] = wraplow(dct_round_shift(temp1));
    step2[13] = wraplow(dct_round_shift(temp2));
    temp1 = (-step1[11] + step1[12]) * COSPI_16_64;
    temp2 = (step1[11] + step1[12]) * COSPI_16_64;
    step2[11] = wraplow(dct_round_shift(temp1));
    step2[12] = wraplow(dct_round_shift(temp2));
    step2[14] = step1[14];
    step2[15] = step1[15];
    step2[16] = wraplow(step1[16] + step1[23]);
    step2[17] = wraplow(step1[17] + step1[22]);
    step2[18] = wraplow(step1[18] + step1[21]);
    step2[19] = wraplow(step1[19] + step1[20]);
    step2[20] = wraplow(step1[19] - step1[20]);
    step2[21] = wraplow(step1[18] - step1[21]);
    step2[22] = wraplow(step1[17] - step1[22]);
    step2[23] = wraplow(step1[16] - step1[23]);
    step2[24] = wraplow(-step1[24] + step1[31]);
    step2[25] = wraplow(-step1[25] + step1[30]);
    step2[26] = wraplow(-step1[26] + step1[29]);
    step2[27] = wraplow(-step1[27] + step1[28]);
    step2[28] = wraplow(step1[27] + step1[28]);
    step2[29] = wraplow(step1[26] + step1[29]);
    step2[30] = wraplow(step1[25] + step1[30]);
    step2[31] = wraplow(step1[24] + step1[31]);
    // stage 7
    step1[0] = wraplow(step2[0] + step2[15]);
    step1[1] = wraplow(step2[1] + step2[14]);
    step1[2] = wraplow(step2[2] + step2[13]);
    step1[3] = wraplow(step2[3] + step2[12]);
    step1[4] = wraplow(step2[4] + step2[11]);
    step1[5] = wraplow(step2[5] + step2[10]);
    step1[6] = wraplow(step2[6] + step2[9]);
    step1[7] = wraplow(step2[7] + step2[8]);
    step1[8] = wraplow(step2[7] - step2[8]);
    step1[9] = wraplow(step2[6] - step2[9]);
    step1[10] = wraplow(step2[5] - step2[10]);
    step1[11] = wraplow(step2[4] - step2[11]);
    step1[12] = wraplow(step2[3] - step2[12]);
    step1[13] = wraplow(step2[2] - step2[13]);
    step1[14] = wraplow(step2[1] - step2[14]);
    step1[15] = wraplow(step2[0] - step2[15]);
    step1[16] = step2[16];
    step1[17] = step2[17];
    step1[18] = step2[18];
    step1[19] = step2[19];
    temp1 = (-step2[20] + step2[27]) * COSPI_16_64;
    temp2 = (step2[20] + step2[27]) * COSPI_16_64;
    step1[20] = wraplow(dct_round_shift(temp1));
    step1[27] = wraplow(dct_round_shift(temp2));
    temp1 = (-step2[21] + step2[26]) * COSPI_16_64;
    temp2 = (step2[21] + step2[26]) * COSPI_16_64;
    step1[21] = wraplow(dct_round_shift(temp1));
    step1[26] = wraplow(dct_round_shift(temp2));
    temp1 = (-step2[22] + step2[25]) * COSPI_16_64;
    temp2 = (step2[22] + step2[25]) * COSPI_16_64;
    step1[22] = wraplow(dct_round_shift(temp1));
    step1[25] = wraplow(dct_round_shift(temp2));
    temp1 = (-step2[23] + step2[24]) * COSPI_16_64;
    temp2 = (step2[23] + step2[24]) * COSPI_16_64;
    step1[23] = wraplow(dct_round_shift(temp1));
    step1[24] = wraplow(dct_round_shift(temp2));
    step1[28] = step2[28];
    step1[29] = step2[29];
    step1[30] = step2[30];
    step1[31] = step2[31];
    // final stage
    output[0] = wraplow(step1[0] + step1[31]);
    output[1] = wraplow(step1[1] + step1[30]);
    output[2] = wraplow(step1[2] + step1[29]);
    output[3] = wraplow(step1[3] + step1[28]);
    output[4] = wraplow(step1[4] + step1[27]);
    output[5] = wraplow(step1[5] + step1[26]);
    output[6] = wraplow(step1[6] + step1[25]);
    output[7] = wraplow(step1[7] + step1[24]);
    output[8] = wraplow(step1[8] + step1[23]);
    output[9] = wraplow(step1[9] + step1[22]);
    output[10] = wraplow(step1[10] + step1[21]);
    output[11] = wraplow(step1[11] + step1[20]);
    output[12] = wraplow(step1[12] + step1[19]);
    output[13] = wraplow(step1[13] + step1[18]);
    output[14] = wraplow(step1[14] + step1[17]);
    output[15] = wraplow(step1[15] + step1[16]);
    output[16] = wraplow(step1[15] - step1[16]);
    output[17] = wraplow(step1[14] - step1[17]);
    output[18] = wraplow(step1[13] - step1[18]);
    output[19] = wraplow(step1[12] - step1[19]);
    output[20] = wraplow(step1[11] - step1[20]);
    output[21] = wraplow(step1[10] - step1[21]);
    output[22] = wraplow(step1[9] - step1[22]);
    output[23] = wraplow(step1[8] - step1[23]);
    output[24] = wraplow(step1[7] - step1[24]);
    output[25] = wraplow(step1[6] - step1[25]);
    output[26] = wraplow(step1[5] - step1[26]);
    output[27] = wraplow(step1[4] - step1[27]);
    output[28] = wraplow(step1[3] - step1[28]);
    output[29] = wraplow(step1[2] - step1[29]);
    output[30] = wraplow(step1[1] - step1[30]);
    output[31] = wraplow(step1[0] - step1[31]);
}
/// Transform type for the 2-D inverse transform (VP9 TX_TYPE order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxKind {
    /// DCT vertically and horizontally.
    DctDct = 0,
    /// ADST vertically (columns), DCT horizontally (rows).
    AdstDct = 1,
    /// DCT vertically, ADST horizontally.
    DctAdst = 2,
    /// ADST both ways.
    AdstAdst = 3,
}

type Xfm1d = fn(&[i64], &mut [i64]);

/// Row/column 1-D transform pair for a `TxKind` (libvpx `IHT_*` tables:
/// `.cols` runs on columns, `.rows` on rows).
fn select_1d(kind: TxKind, dct: Xfm1d, adst: Xfm1d) -> (Xfm1d, Xfm1d) {
    // (cols, rows)
    match kind {
        TxKind::DctDct => (dct, dct),
        TxKind::AdstDct => (adst, dct),
        TxKind::DctAdst => (dct, adst),
        TxKind::AdstAdst => (adst, adst),
    }
}

/// Generic 2-D inverse transform + add (ports of `vp9_iht*_add_c` /
/// `vpx_idct*_add_c`): rows first into a temp, then columns, then
/// round-shift and add to `dest`.
fn iht2d_add(
    input: &[i64],
    dest: &mut [u8],
    dest_off: usize,
    stride: usize,
    n: usize,
    shift: u32,
    cols: Xfm1d,
    rows: Xfm1d,
) {
    let mut out = [0i64; 32 * 32];
    let mut temp_in = [0i64; 32];
    let mut temp_out = [0i64; 32];

    for i in 0..n {
        rows(&input[i * n..], &mut out[i * n..]);
    }
    for i in 0..n {
        for j in 0..n {
            temp_in[j] = out[j * n + i];
        }
        cols(&temp_in, &mut temp_out);
        for j in 0..n {
            let p = dest_off + j * stride + i;
            dest[p] = clip_pixel_add(dest[p], round_pow2(temp_out[j], shift));
        }
    }
}

/// Exact port of `vpx_iwht4x4_16_add_c` (lossless 4x4 Walsh-Hadamard).
pub fn iwht4x4_add(input: &[i64], dest: &mut [u8], dest_off: usize, stride: usize) {
    let mut output = [0i64; 16];
    let mut a1;
    let mut b1;
    let mut c1;
    let mut d1;
    let mut e1;

    for i in 0..4 {
        let ip = &input[i * 4..];
        a1 = ip[0] >> 2;
        c1 = ip[1] >> 2;
        d1 = ip[2] >> 2;
        b1 = ip[3] >> 2;
        a1 += c1;
        d1 -= b1;
        e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= b1;
        d1 += c1;
        output[i * 4] = wraplow(a1);
        output[i * 4 + 1] = wraplow(b1);
        output[i * 4 + 2] = wraplow(c1);
        output[i * 4 + 3] = wraplow(d1);
    }

    for i in 0..4 {
        a1 = output[i];
        c1 = output[4 + i];
        d1 = output[8 + i];
        b1 = output[12 + i];
        a1 += c1;
        d1 -= b1;
        e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= b1;
        d1 += c1;
        let p = dest_off + i;
        dest[p] = clip_pixel_add(dest[p], wraplow(a1));
        dest[p + stride] = clip_pixel_add(dest[p + stride], wraplow(b1));
        dest[p + 2 * stride] = clip_pixel_add(dest[p + 2 * stride], wraplow(c1));
        dest[p + 3 * stride] = clip_pixel_add(dest[p + 3 * stride], wraplow(d1));
    }
}

/// Inverse transform + add for one tx block (`inverse_transform_block_intra`
/// dispatch): `tx_size_log2` is 0..=3 for 4x4..32x32. 32x32 is always
/// DCT_DCT per the VP9 spec.
pub fn inverse_transform_add(
    tx_size_log2: usize,
    kind: TxKind,
    lossless: bool,
    input: &[i64],
    dest: &mut [u8],
    dest_off: usize,
    stride: usize,
) {
    if lossless {
        iwht4x4_add(input, dest, dest_off, stride);
        return;
    }
    match tx_size_log2 {
        0 => {
            let (c, r) = select_1d(kind, idct4, iadst4);
            iht2d_add(input, dest, dest_off, stride, 4, 4, c, r);
        }
        1 => {
            let (c, r) = select_1d(kind, idct8, iadst8);
            iht2d_add(input, dest, dest_off, stride, 8, 5, c, r);
        }
        2 => {
            let (c, r) = select_1d(kind, idct16, iadst16);
            iht2d_add(input, dest, dest_off, stride, 16, 6, c, r);
        }
        _ => {
            iht2d_add(input, dest, dest_off, stride, 32, 6, idct32, idct32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_idct4x4_matches_reference_shortcut() {
        // libvpx vpx_idct4x4_1_add for input[0]=64:
        // out = wraplow(round_shift(64 * 11585)) = wraplow(45) ... compute both
        let mut input = [0i64; 16];
        input[0] = 64;
        let mut dest = vec![128u8; 16];
        inverse_transform_add(0, TxKind::DctDct, false, &input, &mut dest, 0, 4);
        let o1 = wraplow(dct_round_shift(64 * COSPI_16_64));
        let o2 = wraplow(dct_round_shift(o1 * COSPI_16_64));
        let a1 = round_pow2(o2, 4);
        assert!(dest.iter().all(|&p| i64::from(p) == 128 + a1));
    }

    #[test]
    fn wht_dc_only_matches_1_add_variant() {
        let mut input = [0i64; 16];
        input[0] = 16; // ip[0] >> 2 = 4
        let mut dest = vec![100u8; 16];
        iwht4x4_add(&input, &mut dest, 0, 4);
        // vpx_iwht4x4_1_add: a1 = 4, e1 = 2, a1 = 2 -> first col row0 2, rest 1/1/... compute:
        // rows: a1=4,c1=d1=b1=0 -> e1=2, b1=2, c1=2, a1=2, d1=2? recompute:
        // a1=4;c1=0;d1=0;b1=0; a1+=c1=4; d1-=b1=0; e1=(4-0)>>1=2; b1=2-0=2;
        // c1=2-0=2; a1=4-2=2; d1=0+2=2 -> row0 = [2,2,2,2]; other rows zero.
        // cols: each col [2,0,0,0]: a1=2; e1=1; b1=1;c1=1; a1=1; d1=1
        assert!(dest.iter().all(|&p| p == 101));
    }

    #[test]
    fn iadst_zero_input_yields_zero() {
        let input = [0i64; 64];
        let mut out = [7i64; 8];
        iadst8(&input, &mut out);
        assert_eq!(out, [0i64; 8]);
    }
}
