// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! GIF/WebP animation handling: resize, frame-rate reduction, and metrics.
//!
//! This module operates on decoded RGBA pixel data.  It does not perform
//! actual GIF/WebP encoding or decoding; those concerns belong to the
//! container layer.  Instead, it provides pure-Rust algorithms for
//! manipulating animation *frame sequences*:
//!
//! - [`AnimationTransform::resize_frames`] — nearest-neighbour resize of every frame.
//! - [`AnimationTransform::reduce_frames`] — drop frames to achieve a target fps.
//! - [`calculate_animation_metrics`] — compute duration, frame count, avg delay,
//!   and estimated file size.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::animation::{
//!     AnimationFrame, AnimationSpec, AnimationTransform, calculate_animation_metrics,
//! };
//!
//! let frame_pixels = vec![0u8; 4 * 4 * 4]; // 4×4 RGBA
//! let spec = AnimationSpec {
//!     frames: vec![
//!         AnimationFrame { delay_ms: 100, width: 4, height: 4, pixels: frame_pixels.clone() },
//!         AnimationFrame { delay_ms: 100, width: 4, height: 4, pixels: frame_pixels.clone() },
//!     ],
//!     loop_count: 0,
//! };
//!
//! let resized = AnimationTransform::resize_frames(&spec, 2, 2);
//! assert_eq!(resized.frames[0].width, 2);
//! assert_eq!(resized.frames[0].height, 2);
//!
//! let metrics = calculate_animation_metrics(&spec);
//! assert_eq!(metrics.total_duration_ms, 200);
//! assert_eq!(metrics.frame_count, 2);
//! ```

// ---------------------------------------------------------------------------
// AnimationFrame
// ---------------------------------------------------------------------------

/// A single decoded animation frame.
///
/// Pixels are stored in RGBA byte order (4 bytes per pixel, row-major).
/// The length of `pixels` must equal `width * height * 4`; callers are
/// responsible for ensuring this invariant — functions in this module will
/// silently clamp or pad if the slice is under- or over-sized.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationFrame {
    /// How long this frame is displayed before advancing, in milliseconds.
    pub delay_ms: u32,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// RGBA pixel data: `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl AnimationFrame {
    /// Create a new solid-colour frame.
    ///
    /// ```
    /// use oximedia_image_transform::animation::AnimationFrame;
    ///
    /// let frame = AnimationFrame::solid(8, 8, 100, [255, 0, 0, 255]);
    /// assert_eq!(frame.pixels.len(), 8 * 8 * 4);
    /// assert_eq!(frame.pixels[0], 255); // R
    /// assert_eq!(frame.pixels[1], 0);   // G
    /// ```
    pub fn solid(width: u32, height: u32, delay_ms: u32, rgba: [u8; 4]) -> Self {
        let len = (width as usize) * (height as usize) * 4;
        let mut pixels = Vec::with_capacity(len);
        for _ in 0..(width as usize * height as usize) {
            pixels.extend_from_slice(&rgba);
        }
        Self {
            delay_ms,
            width,
            height,
            pixels,
        }
    }

    /// Returns the expected byte length `width * height * 4`.
    pub fn expected_pixel_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// Sample a single RGBA pixel at `(x, y)`.
    ///
    /// Returns `[0, 0, 0, 0]` (transparent black) if `(x, y)` is out of
    /// bounds or the pixel buffer is too short.
    pub fn pixel_at(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        if idx + 3 >= self.pixels.len() {
            return [0, 0, 0, 0];
        }
        [
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        ]
    }
}

// ---------------------------------------------------------------------------
// AnimationSpec
// ---------------------------------------------------------------------------

/// A complete animation consisting of multiple frames.
///
/// ```
/// use oximedia_image_transform::animation::{AnimationFrame, AnimationSpec};
///
/// let spec = AnimationSpec {
///     frames: vec![AnimationFrame::solid(4, 4, 50, [0, 0, 0, 255])],
///     loop_count: 3,
/// };
/// assert_eq!(spec.frames.len(), 1);
/// assert_eq!(spec.loop_count, 3);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationSpec {
    /// Ordered sequence of frames.
    pub frames: Vec<AnimationFrame>,
    /// Number of animation loops.  `0` means infinite looping.
    pub loop_count: u32,
}

impl AnimationSpec {
    /// Create an empty spec.
    pub fn empty() -> Self {
        Self {
            frames: Vec::new(),
            loop_count: 0,
        }
    }

    /// Total animation duration in milliseconds (sum of all frame delays).
    pub fn total_duration_ms(&self) -> u32 {
        self.frames
            .iter()
            .fold(0u32, |acc, f| acc.saturating_add(f.delay_ms))
    }

    /// Effective frames-per-second based on average frame delay.
    ///
    /// Returns `0.0` if the spec has no frames or all delays are zero.
    pub fn fps(&self) -> f32 {
        if self.frames.is_empty() {
            return 0.0;
        }
        let avg_delay = self.total_duration_ms() as f32 / self.frames.len() as f32;
        if avg_delay < f32::EPSILON {
            return 0.0;
        }
        1000.0 / avg_delay
    }
}

// ---------------------------------------------------------------------------
// AnimationTransform
// ---------------------------------------------------------------------------

/// Stateless transform operations on [`AnimationSpec`] values.
pub struct AnimationTransform;

impl AnimationTransform {
    /// Resize every frame to `(target_w, target_h)` using nearest-neighbour sampling.
    ///
    /// Frames with zero source dimensions are replaced by transparent frames of
    /// the target size.  The original `delay_ms` and `loop_count` are preserved.
    ///
    /// ```
    /// use oximedia_image_transform::animation::{AnimationFrame, AnimationSpec, AnimationTransform};
    ///
    /// let frame = AnimationFrame::solid(4, 4, 80, [200, 100, 50, 255]);
    /// let spec = AnimationSpec { frames: vec![frame], loop_count: 1 };
    ///
    /// let resized = AnimationTransform::resize_frames(&spec, 8, 8);
    /// assert_eq!(resized.frames[0].width, 8);
    /// assert_eq!(resized.frames[0].height, 8);
    /// assert_eq!(resized.frames[0].delay_ms, 80);
    /// // Corner pixel should still be the original colour.
    /// assert_eq!(resized.frames[0].pixel_at(0, 0), [200, 100, 50, 255]);
    /// ```
    pub fn resize_frames(spec: &AnimationSpec, target_w: u32, target_h: u32) -> AnimationSpec {
        let target_w = target_w.max(1);
        let target_h = target_h.max(1);

        let frames = spec
            .frames
            .iter()
            .map(|frame| resize_frame(frame, target_w, target_h))
            .collect();

        AnimationSpec {
            frames,
            loop_count: spec.loop_count,
        }
    }

    /// Drop frames so that the effective fps does not exceed `max_fps`.
    ///
    /// Frames are selected by keeping every *N*-th frame where
    /// `N = ceil(current_fps / max_fps)`.  The `delay_ms` of each retained
    /// frame is scaled up proportionally so that the total animation duration
    /// stays approximately constant.
    ///
    /// If `max_fps` is `0.0`, non-positive, or larger than the current fps,
    /// the spec is returned unchanged.
    ///
    /// ```
    /// use oximedia_image_transform::animation::{AnimationFrame, AnimationSpec, AnimationTransform};
    ///
    /// // Build a 10-frame, 100ms-per-frame animation (10 fps).
    /// let frames: Vec<_> = (0..10)
    ///     .map(|_| AnimationFrame::solid(4, 4, 100, [0, 0, 0, 255]))
    ///     .collect();
    /// let spec = AnimationSpec { frames, loop_count: 0 };
    /// assert!((spec.fps() - 10.0).abs() < 0.1);
    ///
    /// // Reduce to ≤5 fps — should halve frame count.
    /// let reduced = AnimationTransform::reduce_frames(&spec, 5.0);
    /// assert!(reduced.frames.len() <= 6);
    /// assert!(reduced.total_duration_ms() >= 900); // total duration preserved
    /// ```
    pub fn reduce_frames(spec: &AnimationSpec, max_fps: f32) -> AnimationSpec {
        if spec.frames.is_empty() || max_fps <= 0.0 {
            return spec.clone();
        }

        let current_fps = spec.fps();
        if current_fps <= max_fps || current_fps < f32::EPSILON {
            return spec.clone();
        }

        // Compute step: keep 1 frame out of every `step`.
        let step = (current_fps / max_fps).ceil() as usize;
        let step = step.max(1);

        let retained: Vec<usize> = (0..spec.frames.len()).step_by(step).collect();
        let scale = step as f32;

        let frames: Vec<AnimationFrame> = retained
            .iter()
            .map(|&i| {
                let src = &spec.frames[i];
                let new_delay = (src.delay_ms as f32 * scale).round() as u32;
                AnimationFrame {
                    delay_ms: new_delay.max(1),
                    width: src.width,
                    height: src.height,
                    pixels: src.pixels.clone(),
                }
            })
            .collect();

        AnimationSpec {
            frames,
            loop_count: spec.loop_count,
        }
    }
}

// ---------------------------------------------------------------------------
// AnimationMetrics
// ---------------------------------------------------------------------------

/// Summary statistics for an [`AnimationSpec`].
///
/// ```
/// use oximedia_image_transform::animation::{AnimationFrame, AnimationSpec, calculate_animation_metrics};
///
/// let spec = AnimationSpec {
///     frames: vec![
///         AnimationFrame::solid(100, 100, 50, [0; 4]),
///         AnimationFrame::solid(100, 100, 150, [0; 4]),
///     ],
///     loop_count: 0,
/// };
///
/// let m = calculate_animation_metrics(&spec);
/// assert_eq!(m.total_duration_ms, 200);
/// assert_eq!(m.frame_count, 2);
/// assert!((m.avg_delay_ms - 100.0).abs() < 1e-3);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationMetrics {
    /// Total animation duration in milliseconds.
    pub total_duration_ms: u32,
    /// Number of frames.
    pub frame_count: usize,
    /// Average delay per frame in milliseconds.
    pub avg_delay_ms: f32,
    /// Rough estimated compressed file size in bytes.
    ///
    /// Computed as `frame_count * avg_pixel_bytes * 0.35` where the compression
    /// ratio of 0.35 is a conservative estimate for animated RGBA content.
    pub estimated_file_size_bytes: usize,
}

/// Compute [`AnimationMetrics`] for the given spec.
pub fn calculate_animation_metrics(spec: &AnimationSpec) -> AnimationMetrics {
    let frame_count = spec.frames.len();

    if frame_count == 0 {
        return AnimationMetrics {
            total_duration_ms: 0,
            frame_count: 0,
            avg_delay_ms: 0.0,
            estimated_file_size_bytes: 0,
        };
    }

    let total_duration_ms = spec.total_duration_ms();
    let avg_delay_ms = total_duration_ms as f32 / frame_count as f32;

    // Estimate uncompressed bytes from per-frame pixel buffers.
    let total_pixel_bytes: usize = spec.frames.iter().map(|f| f.pixels.len()).sum();

    // Apply a rough compression ratio.
    const COMPRESSION_RATIO: f32 = 0.35;
    let estimated_file_size_bytes = (total_pixel_bytes as f32 * COMPRESSION_RATIO) as usize;

    AnimationMetrics {
        total_duration_ms,
        frame_count,
        avg_delay_ms,
        estimated_file_size_bytes,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Nearest-neighbour resize of a single frame.
fn resize_frame(frame: &AnimationFrame, target_w: u32, target_h: u32) -> AnimationFrame {
    let src_w = frame.width.max(1);
    let src_h = frame.height.max(1);

    let expected = src_w as usize * src_h as usize * 4;
    let pixel_len = frame.pixels.len().min(expected);

    let mut out_pixels = Vec::with_capacity(target_w as usize * target_h as usize * 4);

    for dy in 0..target_h {
        // Map target row → source row (nearest-neighbour)
        let sy = ((dy as u64 * src_h as u64) / target_h as u64) as u32;
        let sy = sy.min(src_h - 1);

        for dx in 0..target_w {
            let sx = ((dx as u64 * src_w as u64) / target_w as u64) as u32;
            let sx = sx.min(src_w - 1);

            let src_idx = (sy as usize * src_w as usize + sx as usize) * 4;
            if src_idx + 3 < pixel_len {
                out_pixels.push(frame.pixels[src_idx]);
                out_pixels.push(frame.pixels[src_idx + 1]);
                out_pixels.push(frame.pixels[src_idx + 2]);
                out_pixels.push(frame.pixels[src_idx + 3]);
            } else {
                // Pixel out of range → transparent black
                out_pixels.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    AnimationFrame {
        delay_ms: frame.delay_ms,
        width: target_w,
        height: target_h,
        pixels: out_pixels,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec(frame_count: usize, w: u32, h: u32, delay_ms: u32) -> AnimationSpec {
        let pixels = vec![128u8; w as usize * h as usize * 4];
        let frames = (0..frame_count)
            .map(|_| AnimationFrame {
                delay_ms,
                width: w,
                height: h,
                pixels: pixels.clone(),
            })
            .collect();
        AnimationSpec {
            frames,
            loop_count: 0,
        }
    }

    // ── AnimationFrame ──

    #[test]
    fn test_solid_frame_dimensions() {
        let f = AnimationFrame::solid(10, 20, 100, [255, 0, 0, 255]);
        assert_eq!(f.width, 10);
        assert_eq!(f.height, 20);
        assert_eq!(f.pixels.len(), 10 * 20 * 4);
    }

    #[test]
    fn test_solid_frame_colour() {
        let f = AnimationFrame::solid(2, 2, 50, [1, 2, 3, 4]);
        assert_eq!(f.pixel_at(0, 0), [1, 2, 3, 4]);
        assert_eq!(f.pixel_at(1, 1), [1, 2, 3, 4]);
    }

    #[test]
    fn test_pixel_at_out_of_bounds() {
        let f = AnimationFrame::solid(4, 4, 50, [255, 255, 255, 255]);
        assert_eq!(f.pixel_at(10, 10), [0, 0, 0, 0]);
    }

    #[test]
    fn test_expected_pixel_len() {
        let f = AnimationFrame::solid(8, 6, 100, [0; 4]);
        assert_eq!(f.expected_pixel_len(), 8 * 6 * 4);
    }

    // ── AnimationSpec ──

    #[test]
    fn test_spec_total_duration() {
        let spec = make_spec(5, 4, 4, 100);
        assert_eq!(spec.total_duration_ms(), 500);
    }

    #[test]
    fn test_spec_fps() {
        let spec = make_spec(10, 4, 4, 100); // 10 fps
        assert!((spec.fps() - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_spec_fps_empty() {
        let spec = AnimationSpec::empty();
        assert_eq!(spec.fps(), 0.0);
    }

    // ── AnimationTransform::resize_frames ──

    #[test]
    fn test_resize_frames_dimensions() {
        let spec = make_spec(3, 64, 32, 80);
        let resized = AnimationTransform::resize_frames(&spec, 32, 16);
        for frame in &resized.frames {
            assert_eq!(frame.width, 32);
            assert_eq!(frame.height, 16);
        }
    }

    #[test]
    fn test_resize_preserves_frame_count() {
        let spec = make_spec(5, 10, 10, 50);
        let resized = AnimationTransform::resize_frames(&spec, 5, 5);
        assert_eq!(resized.frames.len(), 5);
    }

    #[test]
    fn test_resize_preserves_delay() {
        let spec = make_spec(2, 8, 8, 150);
        let resized = AnimationTransform::resize_frames(&spec, 4, 4);
        assert_eq!(resized.frames[0].delay_ms, 150);
    }

    #[test]
    fn test_resize_pixel_data_correct_length() {
        let spec = make_spec(1, 16, 16, 100);
        let resized = AnimationTransform::resize_frames(&spec, 8, 4);
        assert_eq!(resized.frames[0].pixels.len(), 8 * 4 * 4);
    }

    #[test]
    fn test_resize_solid_colour_preserved() {
        let frame = AnimationFrame::solid(4, 4, 100, [200, 100, 50, 255]);
        let spec = AnimationSpec {
            frames: vec![frame],
            loop_count: 1,
        };
        let resized = AnimationTransform::resize_frames(&spec, 8, 8);
        // All pixels should still be the same solid colour.
        assert_eq!(resized.frames[0].pixel_at(0, 0), [200, 100, 50, 255]);
        assert_eq!(resized.frames[0].pixel_at(7, 7), [200, 100, 50, 255]);
    }

    #[test]
    fn test_resize_preserves_loop_count() {
        let spec = AnimationSpec {
            frames: vec![],
            loop_count: 5,
        };
        let resized = AnimationTransform::resize_frames(&spec, 10, 10);
        assert_eq!(resized.loop_count, 5);
    }

    // ── AnimationTransform::reduce_frames ──

    #[test]
    fn test_reduce_frames_no_op_when_fps_low() {
        let spec = make_spec(5, 4, 4, 100); // 10 fps
        let reduced = AnimationTransform::reduce_frames(&spec, 24.0); // already below 24
        assert_eq!(reduced.frames.len(), 5);
    }

    #[test]
    fn test_reduce_frames_halves_count() {
        let spec = make_spec(10, 4, 4, 100); // 10 fps
        let reduced = AnimationTransform::reduce_frames(&spec, 5.0);
        assert!(reduced.frames.len() <= 6);
    }

    #[test]
    fn test_reduce_frames_preserves_duration_approx() {
        let spec = make_spec(10, 4, 4, 100); // 1000ms total
        let reduced = AnimationTransform::reduce_frames(&spec, 5.0);
        // Duration should be roughly preserved (within rounding)
        let dur = reduced.total_duration_ms();
        assert!(dur >= 900 && dur <= 1100, "duration={dur}");
    }

    #[test]
    fn test_reduce_frames_zero_fps_returns_unchanged() {
        let spec = make_spec(5, 4, 4, 100);
        let reduced = AnimationTransform::reduce_frames(&spec, 0.0);
        assert_eq!(reduced.frames.len(), 5);
    }

    #[test]
    fn test_reduce_frames_empty_spec() {
        let spec = AnimationSpec::empty();
        let reduced = AnimationTransform::reduce_frames(&spec, 10.0);
        assert_eq!(reduced.frames.len(), 0);
    }

    // ── calculate_animation_metrics ──

    #[test]
    fn test_metrics_empty_spec() {
        let spec = AnimationSpec::empty();
        let m = calculate_animation_metrics(&spec);
        assert_eq!(m.total_duration_ms, 0);
        assert_eq!(m.frame_count, 0);
        assert_eq!(m.avg_delay_ms, 0.0);
        assert_eq!(m.estimated_file_size_bytes, 0);
    }

    #[test]
    fn test_metrics_total_duration() {
        let spec = make_spec(4, 8, 8, 50);
        let m = calculate_animation_metrics(&spec);
        assert_eq!(m.total_duration_ms, 200);
    }

    #[test]
    fn test_metrics_frame_count() {
        let spec = make_spec(7, 4, 4, 100);
        let m = calculate_animation_metrics(&spec);
        assert_eq!(m.frame_count, 7);
    }

    #[test]
    fn test_metrics_avg_delay() {
        let spec = AnimationSpec {
            frames: vec![
                AnimationFrame::solid(4, 4, 100, [0; 4]),
                AnimationFrame::solid(4, 4, 200, [0; 4]),
            ],
            loop_count: 0,
        };
        let m = calculate_animation_metrics(&spec);
        assert!((m.avg_delay_ms - 150.0).abs() < 0.01);
    }

    #[test]
    fn test_metrics_estimated_size_nonzero() {
        let spec = make_spec(3, 64, 64, 100);
        let m = calculate_animation_metrics(&spec);
        assert!(m.estimated_file_size_bytes > 0);
    }
}
