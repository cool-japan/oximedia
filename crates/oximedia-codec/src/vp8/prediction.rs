//! VP8 intra prediction.
//!
//! This module implements intra prediction for VP8, which predicts pixels
//! from reconstructed neighboring pixels within the same frame.
//!
//! VP8 supports several intra prediction modes:
//! - DC prediction: Average of neighboring pixels
//! - Vertical/Horizontal: Copy from above/left
//! - True Motion (TM): Planar prediction
//! - Directional modes (for 4x4 blocks): Various diagonal directions

#![allow(dead_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

// `mb_mode`'s enums are deprecated (0.2.1, defective early seeds superseded
// by the internal RFC 6386 pipeline behind `Vp8Decoder`), but the dispatch
// functions in this file (`predict_intra_16x16`, `predict_intra_4x4`,
// `predict_chroma`) are standalone building blocks that predate that
// pipeline and are not themselves deprecated -- deprecating them would be a
// separate, larger API decision out of scope here. Internal reference site.
#[allow(deprecated)]
use super::mb_mode::{ChromaMode, IntraMode16, IntraMode4};

/// Performs DC prediction for a 16x16 block.
///
/// DC prediction sets all pixels to the average of available top and left neighbors.
///
/// # Arguments
///
/// * `dst` - Destination buffer (16x16 block)
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (16 pixels), or None
/// * `left` - Left neighbor pixels (16 pixels), or None
pub fn predict_dc_16x16(dst: &mut [u8], stride: usize, top: Option<&[u8]>, left: Option<&[u8]>) {
    let dc = calculate_dc_16(top, left);

    for row in 0..16 {
        for col in 0..16 {
            dst[row * stride + col] = dc;
        }
    }
}

/// Performs vertical prediction for a 16x16 block.
///
/// Copies the top row downward.
///
/// # Arguments
///
/// * `dst` - Destination buffer
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (16 pixels)
pub fn predict_v_16x16(dst: &mut [u8], stride: usize, top: &[u8]) {
    for row in 0..16 {
        dst[row * stride..row * stride + 16].copy_from_slice(&top[0..16]);
    }
}

/// Performs horizontal prediction for a 16x16 block.
///
/// Copies the left column rightward.
///
/// # Arguments
///
/// * `dst` - Destination buffer
/// * `stride` - Stride of the destination buffer
/// * `left` - Left neighbor pixels (16 pixels)
pub fn predict_h_16x16(dst: &mut [u8], stride: usize, left: &[u8]) {
    for row in 0..16 {
        let pixel = left[row];
        for col in 0..16 {
            dst[row * stride + col] = pixel;
        }
    }
}

/// Performs True Motion (planar) prediction for a 16x16 block.
///
/// TM prediction uses both top and left neighbors with a gradient.
///
/// # Arguments
///
/// * `dst` - Destination buffer
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (16 pixels)
/// * `left` - Left neighbor pixels (16 pixels)
/// * `top_left` - Top-left corner pixel
#[allow(clippy::similar_names)]
pub fn predict_tm_16x16(dst: &mut [u8], stride: usize, top: &[u8], left: &[u8], top_left: u8) {
    let tl = i32::from(top_left);

    for row in 0..16 {
        let l = i32::from(left[row]);
        for col in 0..16 {
            let t = i32::from(top[col]);
            let pred = l + t - tl;
            dst[row * stride + col] = pred.clamp(0, 255) as u8;
        }
    }
}

/// Predicts a 16x16 block using the specified mode.
///
/// # Arguments
///
/// * `mode` - Intra prediction mode
/// * `dst` - Destination buffer
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (16 pixels), if available
/// * `left` - Left neighbor pixels (16 pixels), if available
/// * `top_left` - Top-left corner pixel, if available
// Takes the deprecated `mb_mode::IntraMode16` by design (see the `use`
// import's note above); internal reference site, not itself deprecated.
#[allow(deprecated)]
pub fn predict_intra_16x16(
    mode: IntraMode16,
    dst: &mut [u8],
    stride: usize,
    top: Option<&[u8]>,
    left: Option<&[u8]>,
    top_left: Option<u8>,
) {
    match mode {
        IntraMode16::DcPred => predict_dc_16x16(dst, stride, top, left),
        IntraMode16::VPred => {
            if let Some(t) = top {
                predict_v_16x16(dst, stride, t);
            } else {
                // Fallback to DC
                predict_dc_16x16(dst, stride, None, left);
            }
        }
        IntraMode16::HPred => {
            if let Some(l) = left {
                predict_h_16x16(dst, stride, l);
            } else {
                // Fallback to DC
                predict_dc_16x16(dst, stride, top, None);
            }
        }
        IntraMode16::TmPred => {
            if let (Some(t), Some(l), Some(tl)) = (top, left, top_left) {
                predict_tm_16x16(dst, stride, t, l, tl);
            } else {
                // Fallback to DC
                predict_dc_16x16(dst, stride, top, left);
            }
        }
    }
}

/// Performs DC prediction for a 4x4 block.
///
/// # Arguments
///
/// * `dst` - Destination buffer (4x4 block)
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (4 pixels), or None
/// * `left` - Left neighbor pixels (4 pixels), or None
pub fn predict_dc_4x4(dst: &mut [u8], stride: usize, top: Option<&[u8]>, left: Option<&[u8]>) {
    let dc = calculate_dc_4(top, left);

    for row in 0..4 {
        for col in 0..4 {
            dst[row * stride + col] = dc;
        }
    }
}

/// Performs vertical prediction for a 4x4 block.
pub fn predict_v_4x4(dst: &mut [u8], stride: usize, top: &[u8]) {
    for row in 0..4 {
        dst[row * stride..row * stride + 4].copy_from_slice(&top[0..4]);
    }
}

/// Performs horizontal prediction for a 4x4 block.
pub fn predict_h_4x4(dst: &mut [u8], stride: usize, left: &[u8]) {
    for row in 0..4 {
        let pixel = left[row];
        for col in 0..4 {
            dst[row * stride + col] = pixel;
        }
    }
}

/// Performs True Motion prediction for a 4x4 block.
#[allow(clippy::similar_names)]
pub fn predict_tm_4x4(dst: &mut [u8], stride: usize, top: &[u8], left: &[u8], top_left: u8) {
    let tl = i32::from(top_left);

    for row in 0..4 {
        let l = i32::from(left[row]);
        for col in 0..4 {
            let t = i32::from(top[col]);
            let pred = l + t - tl;
            dst[row * stride + col] = pred.clamp(0, 255) as u8;
        }
    }
}

/// Predicts a 4x4 block using the specified mode.
///
/// # Arguments
///
/// * `mode` - 4x4 intra prediction mode
/// * `dst` - Destination buffer
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (4 pixels), if available
/// * `left` - Left neighbor pixels (4 pixels), if available
/// * `top_left` - Top-left corner pixel, if available
// Takes the deprecated `mb_mode::IntraMode4` by design (see the `use`
// import's note above); internal reference site, not itself deprecated.
#[allow(deprecated)]
#[allow(clippy::too_many_lines)]
pub fn predict_intra_4x4(
    mode: IntraMode4,
    dst: &mut [u8],
    stride: usize,
    top: Option<&[u8]>,
    left: Option<&[u8]>,
    top_left: Option<u8>,
) {
    match mode {
        IntraMode4::DcPred => predict_dc_4x4(dst, stride, top, left),
        IntraMode4::TmPred => {
            if let (Some(t), Some(l), Some(tl)) = (top, left, top_left) {
                predict_tm_4x4(dst, stride, t, l, tl);
            } else {
                predict_dc_4x4(dst, stride, top, left);
            }
        }
        IntraMode4::VPred => {
            if let Some(t) = top {
                predict_v_4x4(dst, stride, t);
            } else {
                predict_dc_4x4(dst, stride, None, left);
            }
        }
        IntraMode4::HPred => {
            if let Some(l) = left {
                predict_h_4x4(dst, stride, l);
            } else {
                predict_dc_4x4(dst, stride, top, None);
            }
        }
        // For directional modes, we use simplified implementations
        IntraMode4::LdPred
        | IntraMode4::RdPred
        | IntraMode4::VrPred
        | IntraMode4::VlPred
        | IntraMode4::HdPred
        | IntraMode4::HuPred => {
            // Simplified: fall back to DC for now
            // A full implementation would implement each directional mode
            predict_dc_4x4(dst, stride, top, left);
        }
    }
}

/// Predicts chroma block using the specified mode.
///
/// Chroma blocks are 8x8.
///
/// # Arguments
///
/// * `mode` - Chroma prediction mode
/// * `dst` - Destination buffer
/// * `stride` - Stride of the destination buffer
/// * `top` - Top neighbor pixels (8 pixels), if available
/// * `left` - Left neighbor pixels (8 pixels), if available
/// * `top_left` - Top-left corner pixel, if available
// Takes the deprecated `mb_mode::ChromaMode` by design (see the `use`
// import's note above); internal reference site, not itself deprecated.
#[allow(deprecated)]
pub fn predict_chroma(
    mode: ChromaMode,
    dst: &mut [u8],
    stride: usize,
    top: Option<&[u8]>,
    left: Option<&[u8]>,
    top_left: Option<u8>,
) {
    let dc = calculate_dc_8(top, left);

    match mode {
        ChromaMode::DcPred => {
            for row in 0..8 {
                for col in 0..8 {
                    dst[row * stride + col] = dc;
                }
            }
        }
        ChromaMode::VPred => {
            if let Some(t) = top {
                for row in 0..8 {
                    dst[row * stride..row * stride + 8].copy_from_slice(&t[0..8]);
                }
            } else {
                // Fallback to DC
                for row in 0..8 {
                    for col in 0..8 {
                        dst[row * stride + col] = dc;
                    }
                }
            }
        }
        ChromaMode::HPred => {
            if let Some(l) = left {
                for row in 0..8 {
                    let pixel = l[row];
                    for col in 0..8 {
                        dst[row * stride + col] = pixel;
                    }
                }
            } else {
                // Fallback to DC
                for row in 0..8 {
                    for col in 0..8 {
                        dst[row * stride + col] = dc;
                    }
                }
            }
        }
        ChromaMode::TmPred => {
            if let (Some(t), Some(l), Some(tl)) = (top, left, top_left) {
                let tl = i32::from(tl);
                for row in 0..8 {
                    let l_val = i32::from(l[row]);
                    for col in 0..8 {
                        let t_val = i32::from(t[col]);
                        let pred = l_val + t_val - tl;
                        dst[row * stride + col] = pred.clamp(0, 255) as u8;
                    }
                }
            } else {
                // Fallback to DC
                for row in 0..8 {
                    for col in 0..8 {
                        dst[row * stride + col] = dc;
                    }
                }
            }
        }
    }
}

/// Calculates DC value for 16x16 block.
fn calculate_dc_16(top: Option<&[u8]>, left: Option<&[u8]>) -> u8 {
    let (sum, count) = match (top, left) {
        (Some(t), Some(l)) => {
            let sum_top: u32 = t[0..16].iter().map(|&p| u32::from(p)).sum();
            let sum_left: u32 = l[0..16].iter().map(|&p| u32::from(p)).sum();
            (sum_top + sum_left, 32)
        }
        (Some(t), None) => {
            let sum: u32 = t[0..16].iter().map(|&p| u32::from(p)).sum();
            (sum, 16)
        }
        (None, Some(l)) => {
            let sum: u32 = l[0..16].iter().map(|&p| u32::from(p)).sum();
            (sum, 16)
        }
        (None, None) => return 128, // Default DC
    };

    ((sum + count / 2) / count) as u8
}

/// Calculates DC value for 4x4 block.
fn calculate_dc_4(top: Option<&[u8]>, left: Option<&[u8]>) -> u8 {
    let (sum, count) = match (top, left) {
        (Some(t), Some(l)) => {
            let sum_top: u32 = t[0..4].iter().map(|&p| u32::from(p)).sum();
            let sum_left: u32 = l[0..4].iter().map(|&p| u32::from(p)).sum();
            (sum_top + sum_left, 8)
        }
        (Some(t), None) => {
            let sum: u32 = t[0..4].iter().map(|&p| u32::from(p)).sum();
            (sum, 4)
        }
        (None, Some(l)) => {
            let sum: u32 = l[0..4].iter().map(|&p| u32::from(p)).sum();
            (sum, 4)
        }
        (None, None) => return 128, // Default DC
    };

    ((sum + count / 2) / count) as u8
}

/// Calculates DC value for 8x8 block (chroma).
fn calculate_dc_8(top: Option<&[u8]>, left: Option<&[u8]>) -> u8 {
    let (sum, count) = match (top, left) {
        (Some(t), Some(l)) => {
            let sum_top: u32 = t[0..8].iter().map(|&p| u32::from(p)).sum();
            let sum_left: u32 = l[0..8].iter().map(|&p| u32::from(p)).sum();
            (sum_top + sum_left, 16)
        }
        (Some(t), None) => {
            let sum: u32 = t[0..8].iter().map(|&p| u32::from(p)).sum();
            (sum, 8)
        }
        (None, Some(l)) => {
            let sum: u32 = l[0..8].iter().map(|&p| u32::from(p)).sum();
            (sum, 8)
        }
        (None, None) => return 128, // Default DC
    };

    ((sum + count / 2) / count) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_dc_16() {
        let top = [100u8; 16];
        let left = [100u8; 16];

        let dc = calculate_dc_16(Some(&top), Some(&left));
        assert_eq!(dc, 100);

        let dc = calculate_dc_16(Some(&top), None);
        assert_eq!(dc, 100);

        let dc = calculate_dc_16(None, None);
        assert_eq!(dc, 128);
    }

    #[test]
    fn test_calculate_dc_4() {
        let top = [120u8; 4];
        let left = [120u8; 4];

        let dc = calculate_dc_4(Some(&top), Some(&left));
        assert_eq!(dc, 120);

        let dc = calculate_dc_4(None, None);
        assert_eq!(dc, 128);
    }

    #[test]
    fn test_predict_dc_16x16() {
        let mut dst = vec![0u8; 16 * 16];
        let top = [100u8; 16];
        let left = [100u8; 16];

        predict_dc_16x16(&mut dst, 16, Some(&top), Some(&left));

        // All pixels should be 100
        assert!(dst.iter().all(|&p| p == 100));
    }

    #[test]
    fn test_predict_v_16x16() {
        let mut dst = vec![0u8; 16 * 16];
        let top = [50u8; 16];

        predict_v_16x16(&mut dst, 16, &top);

        // All rows should be copies of top
        for row in 0..16 {
            assert!(dst[row * 16..(row + 1) * 16].iter().all(|&p| p == 50));
        }
    }

    #[test]
    fn test_predict_h_16x16() {
        let mut dst = vec![0u8; 16 * 16];
        let mut left = [0u8; 16];
        for i in 0..16 {
            left[i] = i as u8;
        }

        predict_h_16x16(&mut dst, 16, &left);

        // Each row should be filled with its corresponding left pixel
        for row in 0..16 {
            assert!(dst[row * 16..(row + 1) * 16]
                .iter()
                .all(|&p| p == row as u8));
        }
    }

    #[test]
    fn test_predict_tm_16x16() {
        let mut dst = vec![0u8; 16 * 16];
        let top = [100u8; 16];
        let left = [120u8; 16];
        let top_left = 110u8;

        predict_tm_16x16(&mut dst, 16, &top, &left, top_left);

        // Check that prediction was applied (non-zero)
        assert!(dst.iter().any(|&p| p > 0));
    }

    #[test]
    #[allow(deprecated)]
    fn test_predict_intra_16x16_dc() {
        let mut dst = vec![0u8; 16 * 16];
        let top = [100u8; 16];

        predict_intra_16x16(IntraMode16::DcPred, &mut dst, 16, Some(&top), None, None);

        // Should have DC prediction
        assert!(dst.iter().all(|&p| p == 100));
    }

    #[test]
    fn test_predict_dc_4x4() {
        let mut dst = vec![0u8; 4 * 4];
        let top = [50u8; 4];
        let left = [50u8; 4];

        predict_dc_4x4(&mut dst, 4, Some(&top), Some(&left));

        assert!(dst.iter().all(|&p| p == 50));
    }

    #[test]
    fn test_predict_v_4x4() {
        let mut dst = vec![0u8; 4 * 4];
        let top = [60u8; 4];

        predict_v_4x4(&mut dst, 4, &top);

        assert!(dst.iter().all(|&p| p == 60));
    }

    #[test]
    fn test_predict_h_4x4() {
        let mut dst = vec![0u8; 4 * 4];
        let left = [10, 20, 30, 40];

        predict_h_4x4(&mut dst, 4, &left);

        for row in 0..4 {
            assert!(dst[row * 4..(row + 1) * 4].iter().all(|&p| p == left[row]));
        }
    }

    #[test]
    #[allow(deprecated)]
    fn test_predict_intra_4x4() {
        let mut dst = vec![0u8; 4 * 4];
        let top = [80u8; 4];

        predict_intra_4x4(IntraMode4::VPred, &mut dst, 4, Some(&top), None, None);

        assert!(dst.iter().all(|&p| p == 80));
    }

    #[test]
    #[allow(deprecated)]
    fn test_predict_chroma_dc() {
        let mut dst = vec![0u8; 8 * 8];
        let top = [90u8; 8];
        let left = [90u8; 8];

        predict_chroma(
            ChromaMode::DcPred,
            &mut dst,
            8,
            Some(&top),
            Some(&left),
            None,
        );

        assert!(dst.iter().all(|&p| p == 90));
    }

    #[test]
    #[allow(deprecated)]
    fn test_predict_chroma_v() {
        let mut dst = vec![0u8; 8 * 8];
        let top = [70u8; 8];

        predict_chroma(ChromaMode::VPred, &mut dst, 8, Some(&top), None, None);

        assert!(dst.iter().all(|&p| p == 70));
    }
}
