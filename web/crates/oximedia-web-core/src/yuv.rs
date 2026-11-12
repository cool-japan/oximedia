// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Allocation-free YUV <-> RGBA8 conversion kernels.
//!
//! Every kernel is `*_into`: the caller owns the output buffer and the kernel
//! writes into it, never allocating. Loops are written over `chunks_exact` /
//! `chunks_exact_mut` slices so LLVM auto-vectorizes them under `+simd128`
//! while `#![forbid(unsafe_code)]` stays in force.
//!
//! Odd frame widths / heights are handled for 4:2:0: chroma planes are sized
//! with ceiling division (see [`crate::frame::FrameDims`]) and the trailing
//! lone luma column / row shares the last chroma sample.
//!
//! Ported and re-shaped (caller-provided buffers, `ColorMatrix` range control)
//! from the native converters:
//! - `crates/oximedia-core/src/convert/pixel.rs`
//! - `crates/oximedia-core/src/convert/simd_pixel.rs`
//! - `crates/oximedia-simd/src/yuv_rgb.rs`

use crate::error::CoreError;
use crate::frame::FrameDims;
use crate::matrix::{ColorMatrix, RgbToYuv, YuvToRgb};

/// Alpha value written for every fully opaque output pixel.
const OPAQUE: u8 = 255;

/// Returns `Ok` if `actual == expected`, otherwise [`CoreError::BufferLength`].
fn check_len(actual: usize, expected: usize) -> Result<(), CoreError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CoreError::BufferLength { expected, actual })
    }
}

/// Converts an I420 (YUV 4:2:0 planar) frame to packed RGBA8, writing into
/// `dst` (`width * height * 4` bytes). Alpha is set to `255`.
///
/// # Errors
///
/// - [`CoreError::ZeroDimension`] / [`CoreError::DimensionOverflow`] for invalid
///   geometry.
/// - [`CoreError::BufferLength`] if any plane or `dst` has the wrong length.
pub fn i420_to_rgba8_into(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: usize,
    height: usize,
    matrix: ColorMatrix,
    dst: &mut [u8],
) -> Result<(), CoreError> {
    let dims = FrameDims::new(width, height)?;
    check_len(y.len(), dims.luma_len()?)?;
    let chroma_len = dims.chroma_len()?;
    check_len(u.len(), chroma_len)?;
    check_len(v.len(), chroma_len)?;
    check_len(dst.len(), dims.rgba8_len()?)?;

    let cw = dims.chroma_width();
    let coeffs = YuvToRgb::for_matrix(matrix);

    for (row, (dst_row, y_row)) in dst
        .chunks_exact_mut(width * 4)
        .zip(y.chunks_exact(width))
        .enumerate()
    {
        let crow = row / 2;
        let u_row = &u[crow * cw..][..cw];
        let v_row = &v[crow * cw..][..cw];
        for (col, px) in dst_row.chunks_exact_mut(4).enumerate() {
            let ccol = col / 2;
            let (r, g, b) = coeffs.apply(y_row[col], u_row[ccol], v_row[ccol]);
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = OPAQUE;
        }
    }
    Ok(())
}

/// Converts an NV12 (Y plane + interleaved UV plane) frame to packed RGBA8,
/// writing into `dst`. Alpha is set to `255`.
///
/// `uv` is `chroma_width * chroma_height * 2` bytes, ordered `U, V, U, V, ...`.
///
/// # Errors
///
/// As [`i420_to_rgba8_into`].
pub fn nv12_to_rgba8_into(
    y: &[u8],
    uv: &[u8],
    width: usize,
    height: usize,
    matrix: ColorMatrix,
    dst: &mut [u8],
) -> Result<(), CoreError> {
    let dims = FrameDims::new(width, height)?;
    check_len(y.len(), dims.luma_len()?)?;
    check_len(uv.len(), dims.nv12_uv_len()?)?;
    check_len(dst.len(), dims.rgba8_len()?)?;

    let cw = dims.chroma_width();
    let coeffs = YuvToRgb::for_matrix(matrix);

    for (row, (dst_row, y_row)) in dst
        .chunks_exact_mut(width * 4)
        .zip(y.chunks_exact(width))
        .enumerate()
    {
        let crow = row / 2;
        let uv_row = &uv[crow * cw * 2..][..cw * 2];
        for (col, px) in dst_row.chunks_exact_mut(4).enumerate() {
            let ccol = col / 2;
            let (r, g, b) = coeffs.apply(y_row[col], uv_row[ccol * 2], uv_row[ccol * 2 + 1]);
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = OPAQUE;
        }
    }
    Ok(())
}

/// Converts packed RGBA8 to I420 (YUV 4:2:0 planar), writing into the three
/// caller-provided output planes. Alpha is discarded; chroma is subsampled by
/// averaging the available pixels in each 2x2 block (1, 2, or 4 at odd edges).
///
/// # Errors
///
/// - [`CoreError::ZeroDimension`] / [`CoreError::DimensionOverflow`] for invalid
///   geometry.
/// - [`CoreError::BufferLength`] if `rgba` or any output plane has the wrong
///   length.
pub fn rgba8_to_i420_into(
    rgba: &[u8],
    width: usize,
    height: usize,
    matrix: ColorMatrix,
    y_out: &mut [u8],
    u_out: &mut [u8],
    v_out: &mut [u8],
) -> Result<(), CoreError> {
    let dims = FrameDims::new(width, height)?;
    check_len(rgba.len(), dims.rgba8_len()?)?;
    check_len(y_out.len(), dims.luma_len()?)?;
    let chroma_len = dims.chroma_len()?;
    check_len(u_out.len(), chroma_len)?;
    check_len(v_out.len(), chroma_len)?;

    let coeffs = RgbToYuv::for_matrix(matrix);

    // Luma: one output per pixel.
    for (d, px) in y_out.iter_mut().zip(rgba.chunks_exact(4)) {
        *d = coeffs.luma(px[0], px[1], px[2]);
    }

    // Chroma: average each 2x2 block, clamping the block to the frame edges.
    let cw = dims.chroma_width();
    let ch = dims.chroma_height();
    for crow in 0..ch {
        for ccol in 0..cw {
            let mut sum_cb = 0i32;
            let mut sum_cr = 0i32;
            let mut count = 0i32;
            for dr in 0..2 {
                let row = crow * 2 + dr;
                if row >= height {
                    continue;
                }
                for dc in 0..2 {
                    let col = ccol * 2 + dc;
                    if col >= width {
                        continue;
                    }
                    let idx = (row * width + col) * 4;
                    let (_, cb, cr) = coeffs.apply(rgba[idx], rgba[idx + 1], rgba[idx + 2]);
                    sum_cb += i32::from(cb);
                    sum_cr += i32::from(cr);
                    count += 1;
                }
            }
            let half = count / 2;
            let out_idx = crow * cw + ccol;
            u_out[out_idx] = ((sum_cb + half) / count) as u8;
            v_out[out_idx] = ((sum_cr + half) / count) as u8;
        }
    }
    Ok(())
}

/// Extracts the luma (Y) plane from packed RGBA8, writing into `dst`
/// (`width * height` bytes). The `matrix` selects the primaries (BT.601 /
/// BT.709 / BT.2020) and range (limited Y in `[16, 235]` vs full `[0, 255]`).
///
/// # Errors
///
/// - [`CoreError::ZeroDimension`] / [`CoreError::DimensionOverflow`] for invalid
///   geometry.
/// - [`CoreError::BufferLength`] if `rgba` or `dst` has the wrong length.
pub fn rgba8_to_luma_into(
    rgba: &[u8],
    width: usize,
    height: usize,
    matrix: ColorMatrix,
    dst: &mut [u8],
) -> Result<(), CoreError> {
    let dims = FrameDims::new(width, height)?;
    check_len(rgba.len(), dims.rgba8_len()?)?;
    check_len(dst.len(), dims.luma_len()?)?;

    let coeffs = RgbToYuv::for_matrix(matrix);
    for (d, px) in dst.iter_mut().zip(rgba.chunks_exact(4)) {
        *d = coeffs.luma(px[0], px[1], px[2]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ColorMatrix; 5] = [
        ColorMatrix::Bt601Limited,
        ColorMatrix::Bt601Full,
        ColorMatrix::Bt709Limited,
        ColorMatrix::Bt709Full,
        ColorMatrix::Bt2020Limited,
    ];

    // ── Length validation never panics ──────────────────────────────────────

    #[test]
    fn i420_rejects_short_planes() {
        let mut dst = vec![0u8; 4 * 4 * 4];
        let y = vec![16u8; 4 * 4];
        let u = vec![128u8; 2 * 2];
        let short_v = vec![128u8; 2 * 2 - 1];
        assert!(matches!(
            i420_to_rgba8_into(&y, &u, &short_v, 4, 4, ColorMatrix::Bt709Limited, &mut dst),
            Err(CoreError::BufferLength { .. })
        ));
    }

    #[test]
    fn i420_rejects_zero_dimension() {
        let mut dst = vec![0u8; 0];
        assert_eq!(
            i420_to_rgba8_into(&[], &[], &[], 0, 4, ColorMatrix::Bt709Full, &mut dst),
            Err(CoreError::ZeroDimension)
        );
    }

    #[test]
    fn nv12_rejects_wrong_dst() {
        let y = vec![16u8; 4 * 4];
        let uv = vec![128u8; 2 * 2 * 2];
        let mut dst = vec![0u8; 4 * 4 * 4 + 1];
        assert!(matches!(
            nv12_to_rgba8_into(&y, &uv, 4, 4, ColorMatrix::Bt709Limited, &mut dst),
            Err(CoreError::BufferLength { .. })
        ));
    }

    // ── Alpha and grey neutrality ───────────────────────────────────────────

    #[test]
    fn i420_alpha_is_opaque_and_grey_is_neutral() {
        let (w, h) = (8usize, 8usize);
        let y = vec![128u8; w * h];
        let u = vec![128u8; (w / 2) * (h / 2)];
        let v = vec![128u8; (w / 2) * (h / 2)];
        let mut dst = vec![0u8; w * h * 4];
        i420_to_rgba8_into(&y, &u, &v, w, h, ColorMatrix::Bt709Full, &mut dst).unwrap();
        for px in dst.chunks_exact(4) {
            assert_eq!(px[3], 255);
            assert!((i32::from(px[0]) - i32::from(px[1])).abs() <= 2);
            assert!((i32::from(px[1]) - i32::from(px[2])).abs() <= 2);
        }
    }

    // ── Colour anchors: RGBA -> I420 -> RGBA round trip ─────────────────────

    fn round_trip_uniform(matrix: ColorMatrix, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let (w, h) = (4usize, 4usize);
        let mut rgba = vec![0u8; w * h * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = 255;
        }
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; (w / 2) * (h / 2)];
        let mut v = vec![0u8; (w / 2) * (h / 2)];
        rgba8_to_i420_into(&rgba, w, h, matrix, &mut y, &mut u, &mut v).unwrap();

        let mut back = vec![0u8; w * h * 4];
        i420_to_rgba8_into(&y, &u, &v, w, h, matrix, &mut back).unwrap();
        (back[0], back[1], back[2])
    }

    #[test]
    fn colour_anchors_round_trip_within_two() {
        let anchors = [
            (255u8, 255u8, 255u8),
            (0, 0, 0),
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
        ];
        for matrix in ALL {
            for (r, g, b) in anchors {
                let (rr, gg, bb) = round_trip_uniform(matrix, r, g, b);
                assert!(
                    (i32::from(rr) - i32::from(r)).abs() <= 2
                        && (i32::from(gg) - i32::from(g)).abs() <= 2
                        && (i32::from(bb) - i32::from(b)).abs() <= 2,
                    "{matrix:?}: ({r},{g},{b}) -> ({rr},{gg},{bb})"
                );
            }
        }
    }

    // ── Odd-dimension 4:2:0 handling ────────────────────────────────────────

    #[test]
    fn odd_dimensions_i420_round_trip_ok() {
        // 5x3 frame: chroma plane is 3x2 (ceil), covering the odd edge column
        // and row via duplication.
        let (w, h) = (5usize, 3usize);
        let mut rgba = vec![0u8; w * h * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 200;
            px[1] = 100;
            px[2] = 50;
            px[3] = 255;
        }
        let cw = w / 2 + w % 2;
        let ch = h / 2 + h % 2;
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        rgba8_to_i420_into(&rgba, w, h, ColorMatrix::Bt709Full, &mut y, &mut u, &mut v).unwrap();

        let mut back = vec![0u8; w * h * 4];
        i420_to_rgba8_into(&y, &u, &v, w, h, ColorMatrix::Bt709Full, &mut back).unwrap();
        // Uniform colour: every pixel should reconstruct close to the original.
        for px in back.chunks_exact(4) {
            assert!((i32::from(px[0]) - 200).abs() <= 3);
            assert!((i32::from(px[1]) - 100).abs() <= 3);
            assert!((i32::from(px[2]) - 50).abs() <= 3);
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn odd_dimension_luma_extraction() {
        let (w, h) = (3usize, 3usize);
        let mut rgba = vec![0u8; w * h * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
            px[3] = 255;
        }
        let mut luma = vec![0u8; w * h];
        rgba8_to_luma_into(&rgba, w, h, ColorMatrix::Bt709Full, &mut luma).unwrap();
        for &l in &luma {
            assert_eq!(l, 255); // white full-range luma
        }
        rgba8_to_luma_into(&rgba, w, h, ColorMatrix::Bt709Limited, &mut luma).unwrap();
        for &l in &luma {
            assert_eq!(l, 235); // white limited-range luma
        }
    }

    // ── NV12 and I420 agree for the same source ─────────────────────────────

    #[test]
    fn nv12_matches_i420_for_equivalent_planes() {
        let (w, h) = (6usize, 4usize);
        let cw = w / 2;
        let ch = h / 2;
        let mut lcg = Lcg::new(0xC0FFEE);
        let y: Vec<u8> = (0..w * h).map(|_| lcg.next_u8()).collect();
        let u: Vec<u8> = (0..cw * ch).map(|_| lcg.next_u8()).collect();
        let v: Vec<u8> = (0..cw * ch).map(|_| lcg.next_u8()).collect();

        // Interleave into NV12 UV.
        let mut uv = vec![0u8; cw * ch * 2];
        for i in 0..cw * ch {
            uv[i * 2] = u[i];
            uv[i * 2 + 1] = v[i];
        }

        let mut from_i420 = vec![0u8; w * h * 4];
        let mut from_nv12 = vec![0u8; w * h * 4];
        i420_to_rgba8_into(&y, &u, &v, w, h, ColorMatrix::Bt601Limited, &mut from_i420).unwrap();
        nv12_to_rgba8_into(&y, &uv, w, h, ColorMatrix::Bt601Limited, &mut from_nv12).unwrap();
        assert_eq!(from_i420, from_nv12);
    }

    // ── Property: no panic and valid output over pseudo-random frames ────────

    struct Lcg(u64);
    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 32) as u32
        }
        fn next_u8(&mut self) -> u8 {
            (self.next_u32() >> 24) as u8
        }
    }

    #[test]
    fn property_random_i420_roundtrip_no_panic() {
        let mut lcg = Lcg::new(0x1234_5678);
        for _ in 0..64 {
            let w = 2 + (lcg.next_u32() % 15) as usize; // 2..16, includes odd
            let h = 2 + (lcg.next_u32() % 15) as usize;
            let cw = w / 2 + w % 2;
            let ch = h / 2 + h % 2;
            let rgba: Vec<u8> = (0..w * h * 4).map(|_| lcg.next_u8()).collect();

            let mut y = vec![0u8; w * h];
            let mut u = vec![0u8; cw * ch];
            let mut v = vec![0u8; cw * ch];
            rgba8_to_i420_into(&rgba, w, h, ColorMatrix::Bt709Limited, &mut y, &mut u, &mut v)
                .unwrap();

            let mut back = vec![0u8; w * h * 4];
            i420_to_rgba8_into(&y, &u, &v, w, h, ColorMatrix::Bt709Limited, &mut back).unwrap();
            for px in back.chunks_exact(4) {
                assert_eq!(px[3], 255);
            }
        }
    }

    // ── Perf smoke test (ignored by default) ────────────────────────────────

    #[test]
    #[ignore = "perf smoke test; run with --ignored"]
    fn perf_nv12_to_rgba_1080p() {
        use std::time::Instant;
        let (w, h) = (1920usize, 1080usize);
        let y = vec![128u8; w * h];
        let uv = vec![128u8; (w / 2) * (h / 2) * 2];
        let mut dst = vec![0u8; w * h * 4];

        // Warm up (also proves no per-call allocation of the output).
        nv12_to_rgba8_into(&y, &uv, w, h, ColorMatrix::Bt709Limited, &mut dst).unwrap();

        let iters = 30;
        let start = Instant::now();
        for _ in 0..iters {
            nv12_to_rgba8_into(&y, &uv, w, h, ColorMatrix::Bt709Limited, &mut dst).unwrap();
        }
        let elapsed = start.elapsed();
        let per_frame = elapsed / iters;
        println!("nv12->rgba 1920x1080: {per_frame:?}/frame over {iters} iters ({elapsed:?} total)");
    }
}
