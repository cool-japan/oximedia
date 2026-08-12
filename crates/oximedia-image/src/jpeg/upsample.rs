//! Chroma upsampling and colour assembly for decoded JPEG component planes.
//!
//! A subsampled component stores one sample per 2×1, 1×2 or 2×2 block of image
//! pixels. Reconstructing full resolution by replicating samples (box
//! upsampling) is correct in the "not garbage" sense but visibly blocky, and
//! it disagrees with every reference decoder by far more than the error the
//! DCT itself introduces. This module uses the *triangle* filter libjpeg calls
//! fancy upsampling instead: an output sample sits a quarter of a source
//! sample away from its source centre, so the two nearest source samples are
//! weighted 3:1. Both axes are handled in one pass, which makes the 2×2 case
//! exactly libjpeg's `h2v2_fancy_upsample` (`(9a + 3b + 3c + d + 8) / 16`).

use super::decode::Plane;
use crate::error::{ImageError, ImageResult};

/// Upsample every plane to `width × height` and interleave the result.
///
/// Three-component frames are treated as YCbCr and converted to RGB (the JFIF
/// default); one component stays greyscale; any other count is emitted as raw
/// interleaved component samples.
pub(super) fn assemble(width: usize, height: usize, planes: &[Plane]) -> ImageResult<Vec<u8>> {
    let components = planes.len();
    let out_len = crate::limits::checked_dims(width, height, components, 1)
        .map_err(ImageError::InvalidFormat)?;

    if components == 1 {
        return Ok(expand(&planes[0], width, height));
    }

    let expanded: Vec<Vec<u8>> = planes.iter().map(|p| expand(p, width, height)).collect();
    let mut out = vec![0u8; out_len];

    if components == 3 {
        for (i, pixel) in out.chunks_exact_mut(3).enumerate() {
            let (r, g, b) = ycbcr_to_rgb(
                f32::from(expanded[0][i]),
                f32::from(expanded[1][i]),
                f32::from(expanded[2][i]),
            );
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
        }
    } else {
        for (i, pixel) in out.chunks_exact_mut(components).enumerate() {
            for (c, slot) in pixel.iter_mut().enumerate() {
                *slot = expanded[c][i];
            }
        }
    }
    Ok(out)
}

/// Expand one component plane to the full image grid.
fn expand(plane: &Plane, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    // Ratios come from the frame header, where they were checked to be 1 or 2
    // and to divide exactly.
    let scale_x = plane.scale_x.clamp(1, 2);
    let scale_y = plane.scale_y.clamp(1, 2);
    let extent_x = plane.width.max(1);
    let extent_y = plane.height.max(1);

    for y in 0..height {
        let (y_near, y_far, wy_near, wy_far) = taps(y, scale_y, extent_y);
        let row_near = y_near * plane.stride;
        let row_far = y_far * plane.stride;
        let out_row = y * width;
        for x in 0..width {
            let (x_near, x_far, wx_near, wx_far) = taps(x, scale_x, extent_x);
            let near = wx_near * i32::from(plane.samples[row_near + x_near])
                + wx_far * i32::from(plane.samples[row_near + x_far]);
            let far = wx_near * i32::from(plane.samples[row_far + x_near])
                + wx_far * i32::from(plane.samples[row_far + x_far]);
            // Weights sum to 16; +8 rounds to nearest.
            out[out_row + x] = ((wy_near * near + wy_far * far + 8) >> 4) as u8;
        }
    }
    out
}

/// Source taps and weights for one output coordinate.
///
/// Returns `(near, far, weight_near, weight_far)` with weights summing to 4.
/// At `scale == 1` the single tap takes the whole weight, which makes the
/// combined 2-D expression reduce to an exact copy.
#[inline]
fn taps(coord: usize, scale: usize, extent: usize) -> (usize, usize, i32, i32) {
    if scale == 1 {
        let near = coord.min(extent - 1);
        return (near, near, 4, 0);
    }
    let near = (coord / 2).min(extent - 1);
    let far = if coord % 2 == 0 {
        // Output sample sits left/above its source centre.
        near.saturating_sub(1)
    } else {
        (near + 1).min(extent - 1)
    };
    (near, far, 3, 1)
}

/// YCbCr (JFIF, ITU-R BT.601) to RGB with round-to-nearest.
///
/// [`crate::jpeg::ycbcr_to_rgb`] is the public helper and truncates; decoding
/// rounds instead, which removes a systematic half-LSB darkening across the
/// whole image.
#[inline]
fn ycbcr_to_rgb(y: f32, cb: f32, cr: f32) -> (u8, u8, u8) {
    let cb = cb - 128.0;
    let cr = cr - 128.0;
    let r = (y + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
    let g = (y - 0.344_136 * cb - 0.714_136 * cr)
        .round()
        .clamp(0.0, 255.0) as u8;
    let b = (y + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(width: usize, height: usize, scale_x: usize, scale_y: usize, rows: &[&[u8]]) -> Plane {
        let stride = rows[0].len();
        let mut samples = Vec::with_capacity(stride * rows.len());
        for row in rows {
            samples.extend_from_slice(row);
        }
        Plane::for_upsample_test(width, height, scale_x, scale_y, stride, rows.len(), samples)
    }

    #[test]
    fn unscaled_expansion_is_a_crop() {
        // stride 4 but only 3 valid columns: the MCU padding must not leak in.
        let p = plane(3, 2, 1, 1, &[&[10, 20, 30, 99], &[40, 50, 60, 99]]);
        assert_eq!(expand(&p, 3, 2), vec![10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn horizontal_triangle_matches_libjpeg_h2v1() {
        // libjpeg h2v1_fancy_upsample: out[2i] = (3·s[i] + s[i-1] + 2) / 4,
        // out[2i+1] = (3·s[i] + s[i+1] + 1) / 4, edges replicated.
        let p = plane(2, 1, 2, 1, &[&[100, 200]]);
        let out = expand(&p, 4, 1);
        assert_eq!(out[0], 100); // (3·100 + 100 + 2) / 4, edge replicate
        assert_eq!(out[1], 125); // (3·100 + 200 + 2) / 4
        assert_eq!(out[2], 175); // (3·200 + 100 + 2) / 4
        assert_eq!(out[3], 200); // (3·200 + 200 + 2) / 4, edge replicate
    }

    #[test]
    fn vertical_triangle_matches_libjpeg_h1v2() {
        let p = plane(1, 2, 1, 2, &[&[100], &[200]]);
        assert_eq!(expand(&p, 1, 4), vec![100, 125, 175, 200]);
    }

    #[test]
    fn bilinear_expansion_matches_libjpeg_h2v2() {
        // (9a + 3b + 3c + d + 8) / 16 over the four nearest source samples.
        let p = plane(2, 2, 2, 2, &[&[0, 80], &[160, 240]]);
        let out = expand(&p, 4, 4);
        // Corner output: all four taps collapse onto the corner sample.
        assert_eq!(out[0], 0);
        assert_eq!(out[3], 80);
        assert_eq!(out[12], 160);
        assert_eq!(out[15], 240);
        // Interior output (1,1) mixes 0/80/160/240 at 9:3:3:1.
        assert_eq!(out[5], ((9 * 0 + 3 * 80 + 3 * 160 + 240 + 8) / 16) as u8);
        // Interior output (2,2) mixes with 240 dominant.
        assert_eq!(out[10], ((9 * 240 + 3 * 160 + 3 * 80 + 0 + 8) / 16) as u8);
    }

    #[test]
    fn odd_extents_clamp_to_the_real_component_edge() {
        // 3 valid columns for a 5-pixel-wide image: the last output column
        // must reuse column 2, not read the padded column 3.
        let p = plane(3, 1, 2, 1, &[&[10, 20, 30, 200]]);
        let out = expand(&p, 5, 1);
        assert_eq!(out.len(), 5);
        assert!(out.iter().all(|&v| v <= 30), "padding leaked: {out:?}");
    }

    #[test]
    fn neutral_chroma_round_trips_to_grey() {
        let (r, g, b) = ycbcr_to_rgb(128.0, 128.0, 128.0);
        assert_eq!((r, g, b), (128, 128, 128));
    }
}
