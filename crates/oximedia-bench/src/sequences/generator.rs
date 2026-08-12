//! Parameterised synthetic sequence generation.
//!
//! Split out of [`super`] to keep that module under the 2000-line limit.

use crate::{BenchError, BenchResult};
use oximedia_codec::{Plane, VideoFrame};
use oximedia_core::types::{PixelFormat, Rational, Timestamp};

/// Sequence generator for creating synthetic test sequences.
///
/// Unlike the free function [`generate_synthetic_sequence`](super::generate_synthetic_sequence) (which derives
/// its pattern from a [`ContentType`](super::ContentType)), each method here takes its own
/// pattern parameters (an explicit color, a noise amplitude, a velocity
/// vector, ...) and always renders at this generator's configured
/// `width`/`height`/`frame_rate`.
pub struct SequenceGenerator {
    width: usize,
    height: usize,
    frame_rate: Rational,
}

impl SequenceGenerator {
    /// Create a new sequence generator.
    #[must_use]
    pub fn new(width: usize, height: usize, frame_rate: Rational) -> Self {
        Self {
            width,
            height,
            frame_rate,
        }
    }

    /// Per-frame timebase derived from `frame_rate` (seconds-per-frame,
    /// i.e. the reciprocal of `frame_rate`). Falls back to the same
    /// 1/1000 timebase [`generate_synthetic_sequence`](super::generate_synthetic_sequence) uses when
    /// `frame_rate` is degenerate (zero or negative numerator), since
    /// `Rational::new` panics on a zero denominator and a reciprocal of a
    /// zero-numerator rate would produce exactly that.
    fn timebase(&self) -> Rational {
        if self.frame_rate.num > 0 {
            Rational::new(self.frame_rate.den, self.frame_rate.num)
        } else {
            Rational::new(1, 1000)
        }
    }

    /// Generate a solid color sequence.
    ///
    /// Every pixel of every frame is the same RGB color, converted to
    /// limited-range (studio-swing) `Yuv420p` via BT.601 coefficients --
    /// the same black=16/white=235 convention [`generate_synthetic_sequence`](super::generate_synthetic_sequence)'s
    /// `ScreenContent` pattern uses. All frames are byte-identical (zero
    /// spatial *and* zero temporal variance), which is what distinguishes
    /// this from [`Self::generate_gradient`] (nonzero spatial, zero
    /// temporal) and [`Self::generate_noise`] (nonzero spatial *and*
    /// temporal).
    ///
    /// # Errors
    ///
    /// Returns an error if `width` or `height` is zero.
    pub fn generate_solid_color(
        &self,
        frame_count: usize,
        color: (u8, u8, u8),
    ) -> BenchResult<Vec<VideoFrame>> {
        if self.width == 0 || self.height == 0 {
            return Err(BenchError::InvalidConfig(
                "Width and height must be non-zero".to_string(),
            ));
        }

        let (y_val, u_val, v_val) = rgb_to_yuv420_bt601(color.0, color.1, color.2);
        let chroma_w = self.width.div_ceil(2);
        let chroma_h = self.height.div_ceil(2);
        let timebase = self.timebase();

        let mut frames = Vec::with_capacity(frame_count);
        for frame_idx in 0..frame_count {
            let mut frame =
                VideoFrame::new(PixelFormat::Yuv420p, self.width as u32, self.height as u32);
            frame.timestamp = Timestamp::new(frame_idx as i64, timebase);
            frame.planes = vec![
                Plane::with_dimensions(
                    vec![y_val; self.width * self.height],
                    self.width,
                    self.width as u32,
                    self.height as u32,
                ),
                Plane::with_dimensions(
                    vec![u_val; chroma_w * chroma_h],
                    chroma_w,
                    chroma_w as u32,
                    chroma_h as u32,
                ),
                Plane::with_dimensions(
                    vec![v_val; chroma_w * chroma_h],
                    chroma_w,
                    chroma_w as u32,
                    chroma_h as u32,
                ),
            ];
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Generate a gradient sequence.
    ///
    /// Produces a diagonal luma gradient -- `Y` rises from 16 at the
    /// top-left corner to 235 at the bottom-right corner following
    /// `col + row` -- with neutral (128) chroma. All frames are identical
    /// (a static spatial test pattern), which is what distinguishes this
    /// from [`Self::generate_solid_color`] (nonzero spatial variance here
    /// vs. zero there) and from [`Self::generate_noise`] /
    /// [`Self::generate_motion`] (zero temporal variance here vs. nonzero
    /// there).
    ///
    /// # Errors
    ///
    /// Returns an error if `width` or `height` is zero.
    pub fn generate_gradient(&self, frame_count: usize) -> BenchResult<Vec<VideoFrame>> {
        if self.width == 0 || self.height == 0 {
            return Err(BenchError::InvalidConfig(
                "Width and height must be non-zero".to_string(),
            ));
        }

        let chroma_w = self.width.div_ceil(2);
        let chroma_h = self.height.div_ceil(2);
        let timebase = self.timebase();
        let max_diag = (self.width - 1) + (self.height - 1);

        let mut y_plane = vec![0u8; self.width * self.height];
        for row in 0..self.height {
            for col in 0..self.width {
                // `checked_div` naturally covers the degenerate 1x1 case
                // (max_diag == 0) by falling through to the `128` default
                // instead of dividing by zero.
                let luma = (col + row)
                    .checked_mul(235 - 16)
                    .and_then(|scaled| scaled.checked_div(max_diag))
                    .map_or(128, |offset| 16 + offset);
                y_plane[row * self.width + col] = luma as u8;
            }
        }
        let u_plane = vec![128u8; chroma_w * chroma_h];
        let v_plane = vec![128u8; chroma_w * chroma_h];

        let mut frames = Vec::with_capacity(frame_count);
        for frame_idx in 0..frame_count {
            let mut frame =
                VideoFrame::new(PixelFormat::Yuv420p, self.width as u32, self.height as u32);
            frame.timestamp = Timestamp::new(frame_idx as i64, timebase);
            frame.planes = vec![
                Plane::with_dimensions(
                    y_plane.clone(),
                    self.width,
                    self.width as u32,
                    self.height as u32,
                ),
                Plane::with_dimensions(u_plane.clone(), chroma_w, chroma_w as u32, chroma_h as u32),
                Plane::with_dimensions(v_plane.clone(), chroma_w, chroma_w as u32, chroma_h as u32),
            ];
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Generate a noise sequence.
    ///
    /// Every plane is filled with a deterministic pseudo-random pattern
    /// (see `noise_plane`) centered on mid-gray (128) with amplitude
    /// scaled by `noise_level` (clamped to `[0.0, 1.0]`). At
    /// `noise_level == 0.0` every sample is exactly 128 (indistinguishable
    /// from [`Self::generate_solid_color`]'s zero-variance case); as
    /// `noise_level` rises toward `1.0` both the spatial variance *within*
    /// a frame and the temporal variance *between* frames grow, which is
    /// what distinguishes noise from every other pattern in this type.
    ///
    /// # Errors
    ///
    /// Returns an error if `width` or `height` is zero.
    pub fn generate_noise(
        &self,
        frame_count: usize,
        noise_level: f64,
    ) -> BenchResult<Vec<VideoFrame>> {
        if self.width == 0 || self.height == 0 {
            return Err(BenchError::InvalidConfig(
                "Width and height must be non-zero".to_string(),
            ));
        }

        let amplitude = noise_level.clamp(0.0, 1.0) * 119.0;
        let chroma_w = self.width.div_ceil(2);
        let chroma_h = self.height.div_ceil(2);
        let timebase = self.timebase();

        let mut frames = Vec::with_capacity(frame_count);
        for frame_idx in 0..frame_count {
            let mut frame =
                VideoFrame::new(PixelFormat::Yuv420p, self.width as u32, self.height as u32);
            frame.timestamp = Timestamp::new(frame_idx as i64, timebase);
            frame.planes = vec![
                Plane::with_dimensions(
                    noise_plane(
                        self.width,
                        self.height,
                        frame_idx,
                        0x9e37_79b9_7f4a_7c15,
                        128.0,
                        amplitude,
                    ),
                    self.width,
                    self.width as u32,
                    self.height as u32,
                ),
                Plane::with_dimensions(
                    noise_plane(
                        chroma_w,
                        chroma_h,
                        frame_idx,
                        0x6c62_272e_07bb_0142,
                        128.0,
                        amplitude,
                    ),
                    chroma_w,
                    chroma_w as u32,
                    chroma_h as u32,
                ),
                Plane::with_dimensions(
                    noise_plane(
                        chroma_w,
                        chroma_h,
                        frame_idx,
                        0xb492_b66f_be98_f273,
                        128.0,
                        amplitude,
                    ),
                    chroma_w,
                    chroma_w as u32,
                    chroma_h as u32,
                ),
            ];
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Generate a motion sequence (moving object).
    ///
    /// Renders a bright (`Y = 235`) square of side `max(2, min(width,
    /// height) / 8)` on a dark (`Y = 16`) background. The square's
    /// top-left corner at `frame_idx` is `(velocity.0, velocity.1) *
    /// frame_idx`, wrapped with `rem_euclid` so arbitrarily long sequences
    /// stay well-defined. Chroma is neutral (128) throughout. The moving
    /// square is what distinguishes this from every static pattern in this
    /// type ([`Self::generate_solid_color`], [`Self::generate_gradient`],
    /// [`Self::generate_checkerboard`]): its position is a deterministic,
    /// verifiable function of `frame_idx`, unlike [`Self::generate_noise`]'s
    /// unstructured per-pixel variation.
    ///
    /// # Errors
    ///
    /// Returns an error if `width` or `height` is zero.
    pub fn generate_motion(
        &self,
        frame_count: usize,
        velocity: (f64, f64),
    ) -> BenchResult<Vec<VideoFrame>> {
        if self.width == 0 || self.height == 0 {
            return Err(BenchError::InvalidConfig(
                "Width and height must be non-zero".to_string(),
            ));
        }

        let chroma_w = self.width.div_ceil(2);
        let chroma_h = self.height.div_ceil(2);
        let timebase = self.timebase();
        let block_w = (self.width / 8).max(2).min(self.width);
        let block_h = (self.height / 8).max(2).min(self.height);

        let mut frames = Vec::with_capacity(frame_count);
        for frame_idx in 0..frame_count {
            let mut frame =
                VideoFrame::new(PixelFormat::Yuv420p, self.width as u32, self.height as u32);
            frame.timestamp = Timestamp::new(frame_idx as i64, timebase);

            let pos_x = (velocity.0 * frame_idx as f64).rem_euclid(self.width as f64) as usize;
            let pos_y = (velocity.1 * frame_idx as f64).rem_euclid(self.height as f64) as usize;

            let mut y_plane = vec![16u8; self.width * self.height];
            for dy in 0..block_h {
                for dx in 0..block_w {
                    let px = (pos_x + dx) % self.width;
                    let py = (pos_y + dy) % self.height;
                    y_plane[py * self.width + px] = 235;
                }
            }

            frame.planes = vec![
                Plane::with_dimensions(y_plane, self.width, self.width as u32, self.height as u32),
                Plane::with_dimensions(
                    vec![128u8; chroma_w * chroma_h],
                    chroma_w,
                    chroma_w as u32,
                    chroma_h as u32,
                ),
                Plane::with_dimensions(
                    vec![128u8; chroma_w * chroma_h],
                    chroma_w,
                    chroma_w as u32,
                    chroma_h as u32,
                ),
            ];
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Generate a checkerboard pattern sequence.
    ///
    /// Alternating `block_size`x`block_size` black (`Y = 16`) and white
    /// (`Y = 235`) squares, neutral (128) chroma, identical across all
    /// frames -- the same high-frequency spatial pattern
    /// [`generate_synthetic_sequence`](super::generate_synthetic_sequence)'s `ScreenContent` case uses, but
    /// with a caller-chosen block size instead of a fixed 8px cell, which
    /// is what distinguishes different `block_size` calls from each other
    /// (smaller blocks pack more edges into the same frame).
    ///
    /// # Errors
    ///
    /// Returns an error if `width` or `height` is zero.
    pub fn generate_checkerboard(
        &self,
        frame_count: usize,
        block_size: usize,
    ) -> BenchResult<Vec<VideoFrame>> {
        if self.width == 0 || self.height == 0 {
            return Err(BenchError::InvalidConfig(
                "Width and height must be non-zero".to_string(),
            ));
        }

        let block_size = block_size.max(1);
        let chroma_w = self.width.div_ceil(2);
        let chroma_h = self.height.div_ceil(2);
        let timebase = self.timebase();

        let mut y_plane = vec![0u8; self.width * self.height];
        for row in 0..self.height {
            for col in 0..self.width {
                y_plane[row * self.width + col] =
                    if ((col / block_size) + (row / block_size)) % 2 == 0 {
                        235
                    } else {
                        16
                    };
            }
        }
        let u_plane = vec![128u8; chroma_w * chroma_h];
        let v_plane = vec![128u8; chroma_w * chroma_h];

        let mut frames = Vec::with_capacity(frame_count);
        for frame_idx in 0..frame_count {
            let mut frame =
                VideoFrame::new(PixelFormat::Yuv420p, self.width as u32, self.height as u32);
            frame.timestamp = Timestamp::new(frame_idx as i64, timebase);
            frame.planes = vec![
                Plane::with_dimensions(
                    y_plane.clone(),
                    self.width,
                    self.width as u32,
                    self.height as u32,
                ),
                Plane::with_dimensions(u_plane.clone(), chroma_w, chroma_w as u32, chroma_h as u32),
                Plane::with_dimensions(v_plane.clone(), chroma_w, chroma_w as u32, chroma_h as u32),
            ];
            frames.push(frame);
        }
        Ok(frames)
    }
}

// ─── Private synthesis helpers for `SequenceGenerator` ──────────────────────

/// Convert a full-range 8-bit RGB triple to limited-range (studio-swing)
/// `YCbCr` using the integer `BT.601` coefficients (`Y` in `[16, 235]`,
/// `Cb`/`Cr` in `[16, 240]`), matching the black=16/white=235 convention
/// [`generate_synthetic_sequence`](super::generate_synthetic_sequence)'s `ScreenContent` pattern already uses
/// in this module.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgb_to_yuv420_bt601(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));

    let y = 16 + ((66 * r + 129 * g + 25 * b + 128) >> 8);
    let cb = 128 + ((-38 * r - 74 * g + 112 * b + 128) >> 8);
    let cr = 128 + ((112 * r - 94 * g - 18 * b + 128) >> 8);

    (
        y.clamp(0, 255) as u8,
        cb.clamp(0, 255) as u8,
        cr.clamp(0, 255) as u8,
    )
}

/// Deterministic per-pixel pseudo-random plane using a `SplitMix64`-style
/// mixing hash (the same mixing constants [`generate_synthetic_sequence`](super::generate_synthetic_sequence)'s
/// `Sports`/`LiveAction` noise pattern uses) seeded from `(frame_idx,
/// plane_seed, pixel_idx)`, so the result varies both spatially
/// (`pixel_idx`) and temporally (`frame_idx`) while staying perfectly
/// reproducible for a given seed. Each sample is `center` plus a signed
/// offset of magnitude up to `amplitude`, clamped to the valid byte range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn noise_plane(
    width: usize,
    height: usize,
    frame_idx: usize,
    plane_seed: u64,
    center: f64,
    amplitude: f64,
) -> Vec<u8> {
    let mut plane = vec![0u8; width * height];
    for (pixel_idx, sample) in plane.iter_mut().enumerate() {
        let mut seed = (frame_idx as u64)
            .wrapping_mul(plane_seed)
            .wrapping_add(pixel_idx as u64);
        seed ^= seed >> 33;
        seed = seed.wrapping_mul(0xff51_afd7_ed55_8ccd);
        seed ^= seed >> 33;
        seed = seed.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        seed ^= seed >> 33;

        // Top byte of the mixed hash -> uniform fraction in [0.0, 1.0).
        let frac = f64::from((seed >> 56) as u8) / 255.0;
        let value = center + (frac - 0.5) * 2.0 * amplitude;
        *sample = value.clamp(0.0, 255.0) as u8;
    }
    plane
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequence_generator() {
        let gen = SequenceGenerator::new(1920, 1080, Rational::new(30, 1));
        assert_eq!(gen.width, 1920);
        assert_eq!(gen.height, 1080);
    }

    #[test]
    fn test_rgb_to_yuv420_bt601_matches_existing_black_white_convention() {
        // 16 = black, 235 = white is the same limited-range convention that
        // `generate_synthetic_sequence`'s `ScreenContent` pattern hardcodes.
        assert_eq!(rgb_to_yuv420_bt601(255, 255, 255), (235, 128, 128));
        assert_eq!(rgb_to_yuv420_bt601(0, 0, 0), (16, 128, 128));
    }
}
