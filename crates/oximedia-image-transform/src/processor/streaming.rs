// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Tiled streaming image processor.
//!
//! [`StreamingProcessor`] applies transform operations in bounded row-strips
//! (tiles) to keep peak memory at O(tile_rows × width) rather than
//! O(full image height × width).  This is particularly important for very
//! large source images or memory-constrained environments.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::processor::streaming::{StreamingProcessor, StreamingConfig};
//! use oximedia_image_transform::transform::TransformParams;
//!
//! let cfg = StreamingConfig::default();
//! let proc = StreamingProcessor::new(cfg);
//!
//! // Build a tiny 4×4 RGBA image (all red pixels).
//! let width = 4u32;
//! let height = 4u32;
//! let input: Vec<u8> = (0..width * height)
//!     .flat_map(|_| [255u8, 0, 0, 255])
//!     .collect();
//!
//! let mut params = TransformParams::default();
//! params.width = Some(2);
//! params.height = Some(2);
//!
//! let (out, dst_w, dst_h) = proc.process(&input, width, height, &params).expect("process ok");
//! assert_eq!(dst_w, 2);
//! assert_eq!(dst_h, 2);
//! assert_eq!(out.len(), 2 * 2 * 4);
//! ```

use crate::processor::ProcessingError;
use crate::transform::TransformParams;

/// Configuration for the tiled streaming processor.
///
/// Tile rows determine the maximum number of source rows loaded into
/// memory at once.  The overlap provides a halo for filter kernels that
/// read neighbouring rows (e.g. blur).
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Tile height in rows (default 64, minimum 1).
    pub tile_rows: usize,
    /// Overlap rows added to each tile to avoid filter-boundary artefacts
    /// (default 4).  Overlapping rows are computed but discarded in the
    /// output; only the core `tile_rows` rows are written.
    pub overlap_rows: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            tile_rows: 64,
            overlap_rows: 4,
        }
    }
}

/// A streaming image transform processor that limits peak memory usage.
///
/// Each call to [`process`](StreamingProcessor::process) divides the source
/// image into horizontal row-strips and scales each strip independently,
/// then stitches the results into the output buffer.  Because only one
/// tile is held in memory at a time, peak usage is bounded.
///
/// # Limitations
///
/// - Currently performs nearest-neighbour / bilinear scaling only.  For
///   full pipeline operations (blur, sharpen, etc.) use
///   [`apply_transforms`](crate::processor::apply_transforms) which operates
///   on the full in-memory [`PixelBuffer`](crate::processor::PixelBuffer).
/// - Input is expected to be **RGBA8** (4 bytes per pixel, row-major).
#[derive(Debug, Clone)]
pub struct StreamingProcessor {
    config: StreamingConfig,
}

impl StreamingProcessor {
    /// Create a new `StreamingProcessor` with the given configuration.
    pub fn new(config: StreamingConfig) -> Self {
        Self { config }
    }

    /// Apply transform params to RGBA8 `input` data, returning RGBA8 output.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessingError::InvalidDimensions`] if `input.len() !=
    /// src_width * src_height * 4`.
    pub fn process(
        &self,
        input: &[u8],
        src_width: u32,
        src_height: u32,
        params: &TransformParams,
    ) -> Result<(Vec<u8>, u32, u32), ProcessingError> {
        let expected = src_width as usize * src_height as usize * 4;
        if input.len() != expected {
            return Err(ProcessingError::InvalidDimensions {
                width: src_width,
                height: src_height,
                data_len: input.len(),
            });
        }

        let (dst_w, dst_h) = compute_output_dims(src_width, src_height, params);

        if dst_w == 0 || dst_h == 0 {
            return Ok((Vec::new(), 0, 0));
        }

        let src_stride = src_width as usize * 4;
        let dst_stride = dst_w as usize * 4;
        let tile_rows = self.config.tile_rows.max(1);
        let overlap = self.config.overlap_rows;

        let mut output = vec![0u8; dst_w as usize * dst_h as usize * 4];

        let src_h = src_height as usize;
        let n_tiles = (src_h + tile_rows - 1) / tile_rows;

        for tile_idx in 0..n_tiles {
            let src_row_start = tile_idx * tile_rows;
            // Clamp source row end (with overlap, capped at src_h).
            let src_row_end = (src_row_start + tile_rows + overlap).min(src_h);
            let tile_src_rows = src_row_end - src_row_start;

            // Corresponding destination row range (no overlap in output).
            let dst_row_start = (src_row_start as f64 * dst_h as f64 / src_h as f64) as usize;
            let core_dst_end = {
                let next_src = (src_row_start + tile_rows).min(src_h);
                (next_src as f64 * dst_h as f64 / src_h as f64) as usize
            };
            let dst_row_end = core_dst_end.min(dst_h as usize);

            if dst_row_start >= dst_row_end {
                continue;
            }

            let dst_rows_in_tile = dst_row_end - dst_row_start;

            // Extract the source tile.
            let tile_src_start = src_row_start * src_stride;
            let tile_src_end = src_row_end * src_stride;
            let tile_data = &input[tile_src_start..tile_src_end];

            // Scale tile via bilinear sampling.
            let out_tile = scale_tile_bilinear(
                tile_data,
                src_width as usize,
                tile_src_rows,
                dst_w as usize,
                dst_rows_in_tile,
                src_row_start,
                src_h,
                dst_h as usize,
            );

            // Write core (non-overlap) rows into output.
            let out_start = dst_row_start * dst_stride;
            let copy_len = (dst_rows_in_tile * dst_stride).min(out_tile.len());
            let out_end = out_start + copy_len;
            if out_end <= output.len() {
                output[out_start..out_end].copy_from_slice(&out_tile[..copy_len]);
            }
        }

        Ok((output, dst_w, dst_h))
    }
}

/// Compute output dimensions from transform params.
pub(crate) fn compute_output_dims(src_w: u32, src_h: u32, params: &TransformParams) -> (u32, u32) {
    use crate::transform::enforce_aspect_ratio;
    match (params.width, params.height) {
        (Some(w), Some(h)) => enforce_aspect_ratio(src_w, src_h, w, h, params.fit),
        (Some(w), None) => {
            if src_w == 0 {
                return (w, 1);
            }
            let h = (w as f64 * src_h as f64 / src_w as f64).round() as u32;
            (w, h.max(1))
        }
        (None, Some(h)) => {
            if src_h == 0 {
                return (1, h);
            }
            let w = (h as f64 * src_w as f64 / src_h as f64).round() as u32;
            (w.max(1), h)
        }
        (None, None) => (src_w, src_h),
    }
}

/// Bilinear scale a tile strip from source pixel coordinates.
///
/// `tile_row_offset` is the absolute row of the first source tile row
/// within the full source image.  This is needed to map destination rows
/// back to absolute source floating-point coordinates before subtracting
/// the tile offset to find the local index.
fn scale_tile_bilinear(
    tile: &[u8],
    src_w: usize,
    tile_src_rows: usize,
    dst_w: usize,
    dst_rows: usize,
    tile_row_offset: usize,
    total_src_h: usize,
    total_dst_h: usize,
) -> Vec<u8> {
    let src_stride = src_w * 4;
    let dst_stride = dst_w * 4;
    let mut out = vec![0u8; dst_w * dst_rows * 4];

    if total_src_h == 0 || total_dst_h == 0 || src_w == 0 || dst_w == 0 {
        return out;
    }

    for dy in 0..dst_rows {
        // Map output row to absolute source floating-point row.
        let abs_dst_row =
            tile_row_offset + (dy as f64 * (total_src_h as f64 / total_dst_h as f64)) as usize;
        let src_y_f = abs_dst_row as f64 * total_src_h as f64 / total_dst_h as f64;

        // Clamp source y to tile-local coordinates [0, tile_src_rows).
        let local_y0 = (src_y_f as usize).saturating_sub(tile_row_offset);
        let local_y0 = local_y0.min(tile_src_rows.saturating_sub(1));
        let local_y1 = (local_y0 + 1).min(tile_src_rows.saturating_sub(1));
        let frac_y = (src_y_f - src_y_f.floor()).clamp(0.0, 1.0);

        for dx in 0..dst_w {
            // Map output column to source floating-point column.
            let src_x_f = dx as f64 * (src_w as f64 - 1.0) / (dst_w as f64 - 1.0).max(1.0);
            let src_x0 = src_x_f as usize;
            let src_x1 = (src_x0 + 1).min(src_w.saturating_sub(1));
            let frac_x = (src_x_f - src_x_f.floor()).clamp(0.0, 1.0);

            let dst_off = dy * dst_stride + dx * 4;

            // Sample four neighbours (RGBA).
            let p00 = get_pixel(tile, src_stride, local_y0, src_x0);
            let p10 = get_pixel(tile, src_stride, local_y0, src_x1);
            let p01 = get_pixel(tile, src_stride, local_y1, src_x0);
            let p11 = get_pixel(tile, src_stride, local_y1, src_x1);

            if dst_off + 4 <= out.len() {
                for c in 0..4usize {
                    let top = lerp(p00[c], p10[c], frac_x);
                    let bot = lerp(p01[c], p11[c], frac_x);
                    out[dst_off + c] = lerp_f(top, bot, frac_y);
                }
            }
        }
    }
    out
}

/// Read one RGBA pixel at (row, col) from a stride-based slice.
#[inline]
fn get_pixel(data: &[u8], stride: usize, row: usize, col: usize) -> [u8; 4] {
    let off = row * stride + col * 4;
    if off + 4 <= data.len() {
        [data[off], data[off + 1], data[off + 2], data[off + 3]]
    } else {
        [0, 0, 0, 255]
    }
}

/// Linear interpolation between two `u8` values using `t` in [0,1].
#[inline]
fn lerp(a: u8, b: u8, t: f64) -> f64 {
    a as f64 * (1.0 - t) + b as f64 * t
}

/// Clamp-and-round a `f64` back to `u8`.
#[inline]
fn lerp_f(a: f64, b: f64, t: f64) -> u8 {
    (a * (1.0 - t) + b * t).round().clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::{FitMode, TransformParams};

    fn solid_rgba(w: u32, h: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let pixels = w as usize * h as usize;
        (0..pixels).flat_map(|_| [r, g, b, a]).collect()
    }

    #[test]
    fn test_streaming_scale_down() {
        let cfg = StreamingConfig::default();
        let proc = StreamingProcessor::new(cfg);

        let input = solid_rgba(8, 8, 255, 0, 0, 255);
        let mut params = TransformParams::default();
        params.width = Some(4);
        params.height = Some(4);

        let (out, w, h) = proc.process(&input, 8, 8, &params).expect("process ok");
        assert_eq!(w, 4);
        assert_eq!(h, 4);
        assert_eq!(out.len(), 4 * 4 * 4);
        // All pixels should still be red (solid colour, any interpolation).
        for chunk in out.chunks_exact(4) {
            assert_eq!(chunk[0], 255, "R channel must be 255");
            assert_eq!(chunk[1], 0, "G channel must be 0");
            assert_eq!(chunk[2], 0, "B channel must be 0");
            assert_eq!(chunk[3], 255, "A channel must be 255");
        }
    }

    #[test]
    fn test_streaming_identity() {
        let proc = StreamingProcessor::new(StreamingConfig::default());
        let input = solid_rgba(4, 4, 0, 128, 0, 255);
        let params = TransformParams::default();
        let (out, w, h) = proc
            .process(&input, 4, 4, &params)
            .expect("process identity");
        assert_eq!(w, 4);
        assert_eq!(h, 4);
        assert_eq!(out.len(), input.len());
    }

    #[test]
    fn test_streaming_wrong_data_len_is_error() {
        let proc = StreamingProcessor::new(StreamingConfig::default());
        let bad_input = vec![0u8; 100]; // incorrect size
        let params = TransformParams::default();
        assert!(proc.process(&bad_input, 8, 8, &params).is_err());
    }

    #[test]
    fn test_streaming_single_row_tile() {
        // Force tile_rows=1 (one row at a time).
        let cfg = StreamingConfig {
            tile_rows: 1,
            overlap_rows: 0,
        };
        let proc = StreamingProcessor::new(cfg);

        let input = solid_rgba(4, 4, 0, 0, 255, 255);
        let mut params = TransformParams::default();
        params.width = Some(2);
        params.height = Some(2);

        let (out, w, h) = proc
            .process(&input, 4, 4, &params)
            .expect("single-row tiles");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(out.len(), 16);
        // All pixels should be blue.
        for chunk in out.chunks_exact(4) {
            assert_eq!(chunk[2], 255, "B channel must be 255");
        }
    }

    #[test]
    fn test_streaming_aspect_ratio_contain() {
        let proc = StreamingProcessor::new(StreamingConfig::default());
        let input = solid_rgba(16, 8, 200, 100, 50, 255);
        let mut params = TransformParams::default();
        params.width = Some(8);
        params.height = Some(8);
        params.fit = crate::transform::FitMode::Contain;

        let (_, w, h) = proc.process(&input, 16, 8, &params).expect("contain");
        // 16:8 = 2:1 contained in 8×8 → 8×4
        assert_eq!(w, 8);
        assert_eq!(h, 4);
    }

    #[test]
    fn test_compute_output_dims_width_only() {
        let mut p = TransformParams::default();
        p.width = Some(400);
        let (w, h) = compute_output_dims(800, 600, &p);
        assert_eq!(w, 400);
        assert_eq!(h, 300);
    }

    #[test]
    fn test_compute_output_dims_height_only() {
        let mut p = TransformParams::default();
        p.height = Some(300);
        let (w, h) = compute_output_dims(800, 600, &p);
        assert_eq!(w, 400);
        assert_eq!(h, 300);
    }

    #[test]
    fn test_compute_output_dims_none() {
        let p = TransformParams::default();
        let (w, h) = compute_output_dims(800, 600, &p);
        assert_eq!(w, 800);
        assert_eq!(h, 600);
    }

    #[test]
    fn test_compute_output_dims_cover() {
        let mut p = TransformParams::default();
        p.width = Some(400);
        p.height = Some(400);
        p.fit = FitMode::Cover;
        let (w, h) = compute_output_dims(800, 400, &p);
        // 800:400 = 2:1, covering 400×400 → scale by max(400/800, 400/400) = 1
        assert!(
            w >= 400 && h >= 400,
            "cover output {w}×{h} must be ≥ 400×400"
        );
    }
}
