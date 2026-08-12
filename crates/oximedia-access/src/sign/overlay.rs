//! Sign language video overlay.
//!
//! [`SignLanguageOverlay::apply`] composites a sign-language interpreter's
//! video (`sign_frame`) as a picture-in-picture region onto the main video
//! (`main_frame`): scaled to the configured `SignSize`, placed per the
//! configured [`SignPosition`], framed with the configured [`SignBorder`],
//! and alpha-blended at `config.opacity`. Both frames must be
//! [`PixelFormat::Yuv420p`] — the only pixel layout this compositor
//! understands today; anything else is an honest [`AccessError`] rather than
//! a silently wrong composite.
//!
//! # Chroma alignment
//!
//! 4:2:0 chroma samples one U/V pair per 2×2 luma block, so every geometric
//! quantity that decides *where* the overlay sits — its outer position,
//! its outer size, and the border ring's thickness — is floored to an even
//! number of luma pixels before anything is painted. That guarantees the
//! overlay's edges always land exactly on a chroma-sample boundary; without
//! it, an overlay whose left edge fell on an odd luma column would still
//! have to color a *whole* 2×2 chroma block (half inside the overlay, half
//! not), producing a one-pixel color fringe that has nothing to do with
//! either image. See `round_down_even` and the geometry tests below.
//!
//! The [`SignBorderStyle::Rounded`] corner mask (`point_allowed`) is
//! evaluated once per *chroma sample*, at that sample's top-left luma
//! coordinate — so the rounded arc quantizes to the chroma grid, not the
//! luma grid. That is a real, if coarser, geometric rounding rather than an
//! approximation, and is what keeps a chroma block from being split between
//! "inside the rounded corner" and "outside" it.

use crate::error::{AccessError, AccessResult};
use crate::sign::{SignBorder, SignBorderStyle, SignConfig, SignPosition};
use oximedia_core::traits::VideoFrame;
use oximedia_core::PixelFormat;

/// Margin, in pixels, kept between a corner-positioned overlay and the main
/// frame's edge. Always used through [`round_down_even`], so it never
/// itself introduces an odd offset.
const OVERLAY_MARGIN_PX: u32 = 16;

/// Sign language overlay manager.
#[derive(Debug, Clone)]
pub struct SignLanguageOverlay {
    config: SignConfig,
}

impl SignLanguageOverlay {
    /// Create a new sign language overlay.
    #[must_use]
    pub const fn new(config: SignConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration.
    #[must_use]
    pub fn default() -> Self {
        Self::new(SignConfig::default())
    }

    /// Composite the sign-language video onto the main video frame.
    ///
    /// Both frames must be [`PixelFormat::Yuv420p`] with exactly 3 planes
    /// and matching strides — the only layout this compositor understands.
    /// The sign-language frame is scaled (nearest-neighbor) to the
    /// configured `SignSize` while preserving its own aspect ratio,
    /// placed per the configured [`SignPosition`], framed with the
    /// configured [`SignBorder`] if any, and alpha-blended over `main_frame`
    /// at `config.opacity`. See the [module docs](self) for the chroma
    /// alignment rule this applies to every geometric quantity involved.
    ///
    /// # Errors
    ///
    /// Returns [`AccessError::SignLanguageFailed`] if `config.opacity` is
    /// out of range (see [`Self::validate`]), if either frame is not
    /// [`PixelFormat::Yuv420p`] or does not carry exactly 3 planes/strides,
    /// or if either frame has zero width or height.
    pub fn apply(
        &self,
        main_frame: &VideoFrame,
        sign_frame: &VideoFrame,
    ) -> AccessResult<VideoFrame> {
        self.validate()?;
        Self::validate_yuv420p_frame(main_frame, "main")?;
        Self::validate_yuv420p_frame(sign_frame, "sign-language")?;

        let geometry = self.compute_geometry(
            main_frame.width,
            main_frame.height,
            sign_frame.width,
            sign_frame.height,
        )?;

        let mut planes = main_frame.planes.clone();

        if let Some(border) = self.config.border {
            if border.style != SignBorderStyle::None && border.width > 0 {
                self.paint_border(
                    &mut planes,
                    &main_frame.strides,
                    main_frame.width,
                    main_frame.height,
                    &geometry,
                    &border,
                );
            }
        }

        let (border_style, border_radius) = self
            .config
            .border
            .map_or((SignBorderStyle::None, 0), |border| {
                (border.style, border.radius)
            });
        self.paint_video(
            &mut planes,
            &main_frame.strides,
            sign_frame,
            border_style,
            border_radius,
            &geometry,
        );

        Ok(VideoFrame::new(
            main_frame.format,
            main_frame.width,
            main_frame.height,
            main_frame.timestamp,
            planes,
            main_frame.strides.clone(),
            main_frame.is_keyframe,
        ))
    }

    /// Get configuration.
    #[must_use]
    pub const fn config(&self) -> &SignConfig {
        &self.config
    }

    /// Update configuration.
    pub fn set_config(&mut self, config: SignConfig) {
        self.config = config;
    }

    /// Validate configuration.
    pub fn validate(&self) -> AccessResult<()> {
        if !(0.0..=1.0).contains(&self.config.opacity) {
            return Err(AccessError::SignLanguageFailed(
                "Opacity must be between 0.0 and 1.0".to_string(),
            ));
        }

        Ok(())
    }

    // ── Frame validation ─────────────────────────────────────────────────────

    /// Check that `frame` is a well-formed [`PixelFormat::Yuv420p`] frame:
    /// the right format, exactly 3 planes/strides, and non-zero dimensions.
    /// `label` names the frame in the error message (`"main"` or
    /// `"sign-language"`).
    fn validate_yuv420p_frame(frame: &VideoFrame, label: &str) -> AccessResult<()> {
        if frame.format != PixelFormat::Yuv420p {
            return Err(AccessError::SignLanguageFailed(format!(
                "PiP compositing only supports Yuv420p {label} frames today, got {:?}",
                frame.format
            )));
        }
        if frame.planes.len() != 3 || frame.strides.len() != 3 {
            return Err(AccessError::SignLanguageFailed(format!(
                "{label} frame claims Yuv420p but has {} plane(s)/{} stride(s), expected 3 each",
                frame.planes.len(),
                frame.strides.len()
            )));
        }
        if frame.width == 0 || frame.height == 0 {
            return Err(AccessError::SignLanguageFailed(format!(
                "{label} frame has zero width or height"
            )));
        }
        Ok(())
    }

    // ── Geometry ──────────────────────────────────────────────────────────────

    /// Compute the overlay's placement and size for a `main_w`x`main_h`
    /// destination and a `sign_w`x`sign_h` source, honouring
    /// `self.config`'s position/size/border. Every field of the result is
    /// even — see the [module docs](self).
    fn compute_geometry(
        &self,
        main_w: u32,
        main_h: u32,
        sign_w: u32,
        sign_h: u32,
    ) -> AccessResult<OverlayGeometry> {
        if main_w < 4 || main_h < 4 {
            return Err(AccessError::SignLanguageFailed(format!(
                "main frame {main_w}x{main_h} is too small to composite an overlay onto"
            )));
        }

        // Video content size: `SignSize` percent of main width, aspect
        // ratio preserved from the sign-language source, then floored to
        // even pixels on both axes.
        let percent = u32::from(self.config.size.as_percent()).clamp(1, 100);
        let raw_w = (main_w * percent / 100).max(2);
        let raw_h = ((u64::from(raw_w) * u64::from(sign_h)) / u64::from(sign_w)).max(2);
        let raw_h = u32::try_from(raw_h).unwrap_or(main_h);
        let mut video_w = round_down_even(raw_w).max(2);
        let mut video_h = round_down_even(raw_h).max(2);

        // Border ring thickness, rounded to even for the same reason.
        let border_px = self
            .config
            .border
            .filter(|border| border.style != SignBorderStyle::None)
            .map_or(0, |border| round_down_even(border.width));

        // Shrink the video (never the border) so `video + 2*border` always
        // fits inside the main frame.
        let max_outer_w = round_down_even(main_w);
        let max_outer_h = round_down_even(main_h);
        if video_w + 2 * border_px > max_outer_w {
            video_w = round_down_even(max_outer_w.saturating_sub(2 * border_px)).max(2);
        }
        if video_h + 2 * border_px > max_outer_h {
            video_h = round_down_even(max_outer_h.saturating_sub(2 * border_px)).max(2);
        }

        let outer_w = video_w + 2 * border_px;
        let outer_h = video_h + 2 * border_px;

        // Position: corner presets keep a fixed margin from the edge;
        // `Custom(x_pct, y_pct)` places the overlay's top-left
        // proportionally across the space that remains once its footprint
        // is subtracted, so 0% and 100% both stay fully on-frame. Every
        // result is floored to even.
        let free_w = main_w.saturating_sub(outer_w);
        let free_h = main_h.saturating_sub(outer_h);
        let margin_x = OVERLAY_MARGIN_PX.min(free_w);
        let margin_y = OVERLAY_MARGIN_PX.min(free_h);
        let (raw_x, raw_y) = match self.config.position {
            SignPosition::TopLeft => (margin_x, margin_y),
            SignPosition::TopRight => (free_w.saturating_sub(margin_x), margin_y),
            SignPosition::BottomLeft => (margin_x, free_h.saturating_sub(margin_y)),
            SignPosition::BottomRight => (
                free_w.saturating_sub(margin_x),
                free_h.saturating_sub(margin_y),
            ),
            SignPosition::Custom(x_pct, y_pct) => (
                free_w * u32::from(x_pct.min(100)) / 100,
                free_h * u32::from(y_pct.min(100)) / 100,
            ),
        };
        let outer_x = round_down_even(raw_x.min(free_w));
        let outer_y = round_down_even(raw_y.min(free_h));

        Ok(OverlayGeometry {
            border_px,
            outer_x,
            outer_y,
            outer_w,
            outer_h,
            video_x: outer_x + border_px,
            video_y: outer_y + border_px,
            video_w,
            video_h,
        })
    }

    // ── Painting ──────────────────────────────────────────────────────────────

    /// Paint `border`'s ring into `planes`, alpha-blended by
    /// `config.opacity * (border.color.alpha / 255)`.
    fn paint_border(
        &self,
        planes: &mut [Vec<u8>],
        strides: &[usize],
        main_w: u32,
        main_h: u32,
        geometry: &OverlayGeometry,
        border: &SignBorder,
    ) {
        if geometry.border_px == 0 {
            return;
        }
        let (color_r, color_g, color_b, color_a) = border.color;
        let (plane_y, plane_u, plane_v) = rgb_to_yuv(color_r, color_g, color_b);
        let alpha = self.config.opacity.clamp(0.0, 1.0) * (f32::from(color_a) / 255.0);
        if alpha <= 0.0 {
            return;
        }

        for plane_index in 0..3usize {
            let (sub_x, sub_y) = plane_subsampling(plane_index);
            let plane_value = match plane_index {
                0 => plane_y,
                1 => plane_u,
                _ => plane_v,
            };
            let Some(&stride) = strides.get(plane_index) else {
                continue;
            };
            let plane_w = main_w / sub_x;
            let plane_h = main_h / sub_y;
            let Some(buf) = planes.get_mut(plane_index) else {
                continue;
            };

            // Bound the scan to the outer (border+video) box's own
            // footprint in this plane's coordinates — not the whole frame.
            // `in_ring` below still has to reject the video interior, but
            // there is no reason to visit pixels the ring could never reach
            // in the first place; on a real frame the ring is a few
            // thousand pixels out of a multi-megapixel plane.
            let px0 = (geometry.outer_x / sub_x).min(plane_w);
            let py0 = (geometry.outer_y / sub_y).min(plane_h);
            let px1 = ((geometry.outer_x + geometry.outer_w) / sub_x).min(plane_w);
            let py1 = ((geometry.outer_y + geometry.outer_h) / sub_y).min(plane_h);

            for py in py0..py1 {
                let luma_y = py * sub_y;
                for px in px0..px1 {
                    let luma_x = px * sub_x;
                    if !in_ring(luma_x, luma_y, geometry) {
                        continue;
                    }
                    if !point_allowed(luma_x, luma_y, geometry, border.style, border.radius) {
                        continue;
                    }
                    let idx = py as usize * stride + px as usize;
                    if let Some(dst) = buf.get_mut(idx) {
                        *dst = blend_u8(*dst, plane_value, alpha);
                    }
                }
            }
        }
    }

    /// Scale `sign` (nearest-neighbor) into `geometry`'s video region and
    /// alpha-blend it into `planes` at `config.opacity`, skipping any pixel
    /// the rounded-corner mask excludes.
    fn paint_video(
        &self,
        planes: &mut [Vec<u8>],
        strides: &[usize],
        sign: &VideoFrame,
        border_style: SignBorderStyle,
        border_radius: u32,
        geometry: &OverlayGeometry,
    ) {
        let alpha = self.config.opacity.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }

        for plane_index in 0..3usize {
            let (sub_x, sub_y) = plane_subsampling(plane_index);
            let Some(&dst_stride) = strides.get(plane_index) else {
                continue;
            };
            let Some(&src_stride) = sign.strides.get(plane_index) else {
                continue;
            };
            let Some(src_plane) = sign.planes.get(plane_index) else {
                continue;
            };

            let dst_x0 = geometry.video_x / sub_x;
            let dst_y0 = geometry.video_y / sub_y;
            let dst_w = (geometry.video_w / sub_x).max(1);
            let dst_h = (geometry.video_h / sub_y).max(1);
            let src_w = (sign.width / sub_x).max(1);
            let src_h = (sign.height / sub_y).max(1);

            let Some(dst_plane) = planes.get_mut(plane_index) else {
                continue;
            };

            for dy in 0..dst_h {
                let dst_y = dst_y0 + dy;
                let src_y = (dy * src_h) / dst_h;
                for dx in 0..dst_w {
                    let dst_x = dst_x0 + dx;
                    if !point_allowed(
                        dst_x * sub_x,
                        dst_y * sub_y,
                        geometry,
                        border_style,
                        border_radius,
                    ) {
                        continue;
                    }
                    let src_x = (dx * src_w) / dst_w;
                    let src_idx = src_y as usize * src_stride + src_x as usize;
                    let dst_idx = dst_y as usize * dst_stride + dst_x as usize;
                    let Some(&src_val) = src_plane.get(src_idx) else {
                        continue;
                    };
                    let Some(dst_val) = dst_plane.get_mut(dst_idx) else {
                        continue;
                    };
                    *dst_val = blend_u8(*dst_val, src_val, alpha);
                }
            }
        }
    }
}

// ── Geometry ─────────────────────────────────────────────────────────────────

/// Placement and size of a composited overlay, in main-frame pixel
/// coordinates. Every field is even — see the [module docs](self).
#[derive(Debug, Clone, Copy)]
struct OverlayGeometry {
    /// Border ring thickness in pixels (0 when there is no border).
    border_px: u32,
    /// Top-left of the border ring (or of the video, when `border_px == 0`).
    outer_x: u32,
    outer_y: u32,
    /// Footprint of border ring + video content together.
    outer_w: u32,
    outer_h: u32,
    /// Top-left of the video content itself.
    video_x: u32,
    video_y: u32,
    /// Size the sign-language frame is scaled to.
    video_w: u32,
    video_h: u32,
}

/// Floor `value` to the nearest even number.
const fn round_down_even(value: u32) -> u32 {
    value & !1
}

/// Chroma subsampling factors for [`PixelFormat::Yuv420p`] plane
/// `plane_index`: `(1, 1)` for the luma plane (0), `(2, 2)` for either
/// chroma plane (1, 2).
const fn plane_subsampling(plane_index: usize) -> (u32, u32) {
    if plane_index == 0 {
        (1, 1)
    } else {
        (2, 2)
    }
}

/// Whether main-frame luma pixel `(x, y)` is inside `geometry`'s outer
/// footprint but *outside* its video-content box — the border ring.
const fn in_ring(x: u32, y: u32, geometry: &OverlayGeometry) -> bool {
    let in_outer = x >= geometry.outer_x
        && x < geometry.outer_x + geometry.outer_w
        && y >= geometry.outer_y
        && y < geometry.outer_y + geometry.outer_h;
    if !in_outer {
        return false;
    }
    let in_video = x >= geometry.video_x
        && x < geometry.video_x + geometry.video_w
        && y >= geometry.video_y
        && y < geometry.video_y + geometry.video_h;
    !in_video
}

/// Whether the main-frame luma pixel at `(x, y)` should be painted, given
/// `geometry`'s outer footprint and a [`SignBorderStyle::Rounded`] corner
/// `radius`.
///
/// For [`SignBorderStyle::Solid`] or [`SignBorderStyle::None`] every pixel
/// inside the outer footprint is allowed — there are no corners to cut. For
/// [`SignBorderStyle::Rounded`], a pixel within `radius` of one of the four
/// outer corners is allowed only if it also falls inside that corner's
/// quarter-circle: a real geometric distance check against the corner
/// center, applied to the *whole* outer footprint (border ring and video
/// content alike), so the border ring and the video it frames always trace
/// the same rounded outline.
fn point_allowed(
    x: u32,
    y: u32,
    geometry: &OverlayGeometry,
    style: SignBorderStyle,
    radius: u32,
) -> bool {
    if style != SignBorderStyle::Rounded || radius == 0 {
        return true;
    }
    let radius = radius.min(geometry.outer_w / 2).min(geometry.outer_h / 2);
    if radius == 0 {
        return true;
    }
    let left = geometry.outer_x;
    let top = geometry.outer_y;
    let right = geometry.outer_x + geometry.outer_w;
    let bottom = geometry.outer_y + geometry.outer_h;

    let center = if x < left + radius && y < top + radius {
        Some((left + radius, top + radius))
    } else if x + radius >= right && y < top + radius {
        Some((right - radius, top + radius))
    } else if x < left + radius && y + radius >= bottom {
        Some((left + radius, bottom - radius))
    } else if x + radius >= right && y + radius >= bottom {
        Some((right - radius, bottom - radius))
    } else {
        None
    };

    match center {
        None => true,
        Some((cx, cy)) => {
            let dx = f64::from(x) - f64::from(cx);
            let dy = f64::from(y) - f64::from(cy);
            (dx * dx + dy * dy).sqrt() <= f64::from(radius)
        }
    }
}

/// Convert an RGB color to BT.601 full-range YUV.
///
/// This is the standard broadcast conversion (the same coefficients
/// `libswscale`/`ffmpeg` use in full-range mode) — not a fabrication, but it
/// is an assumption worth stating: [`PixelFormat::Yuv420p`] carries no
/// explicit full-vs-limited-range flag in this crate's model, so full-range
/// is what this compositor assumes when rendering a [`SignBorder`] color
/// into the YUV planes.
fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = -0.168_736 * r - 0.331_264 * g + 0.5 * b + 128.0;
    let v = 0.5 * r - 0.418_688 * g - 0.081_312 * b + 128.0;
    (
        y.round().clamp(0.0, 255.0) as u8,
        u.round().clamp(0.0, 255.0) as u8,
        v.round().clamp(0.0, 255.0) as u8,
    )
}

/// Straight-alpha blend `src` over `dst`, `alpha` in `[0.0, 1.0]`.
fn blend_u8(dst: u8, src: u8, alpha: f32) -> u8 {
    let out = f32::from(src) * alpha + f32::from(dst) * (1.0 - alpha);
    out.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::SignSize;
    use oximedia_core::{Rational, Timestamp};

    // ── Test frame builders ──────────────────────────────────────────────────

    fn timestamp() -> Timestamp {
        Timestamp::new(0, Rational::new(1, 1000))
    }

    /// A Yuv420p frame with a position-dependent (not uniform) pattern per
    /// plane, so a wrong row/column index, a swapped U/V plane, or a
    /// misplaced region shows up as a wrong *value*, not just "some value".
    fn patterned_frame(width: u32, height: u32) -> VideoFrame {
        let cw = width / 2;
        let ch = height / 2;
        let mut y_plane = vec![0u8; (width * height) as usize];
        for row in 0..height {
            for col in 0..width {
                y_plane[(row * width + col) as usize] = ((col * 7 + row * 13) % 251) as u8;
            }
        }
        let mut u_plane = vec![0u8; (cw * ch) as usize];
        let mut v_plane = vec![0u8; (cw * ch) as usize];
        for row in 0..ch {
            for col in 0..cw {
                let idx = (row * cw + col) as usize;
                u_plane[idx] = ((col * 3 + row * 5) % 251) as u8;
                v_plane[idx] = ((col * 11 + row * 17 + 1) % 251) as u8;
            }
        }
        VideoFrame::new(
            PixelFormat::Yuv420p,
            width,
            height,
            timestamp(),
            vec![y_plane, u_plane, v_plane],
            vec![width as usize, cw as usize, cw as usize],
            true,
        )
    }

    /// A Yuv420p frame filled with one constant value per plane (all three
    /// planes different), for exact-copy / blend-factor assertions.
    fn solid_frame(width: u32, height: u32, y: u8, u: u8, v: u8) -> VideoFrame {
        let cw = width / 2;
        let ch = height / 2;
        VideoFrame::new(
            PixelFormat::Yuv420p,
            width,
            height,
            timestamp(),
            vec![
                vec![y; (width * height) as usize],
                vec![u; (cw * ch) as usize],
                vec![v; (cw * ch) as usize],
            ],
            vec![width as usize, cw as usize, cw as usize],
            true,
        )
    }

    fn config(position: SignPosition, size: SignSize, border: Option<SignBorder>) -> SignConfig {
        SignConfig {
            position,
            size,
            border,
            opacity: 1.0,
        }
    }

    // ── Pre-existing behaviour (unchanged) ──────────────────────────────────

    #[test]
    fn test_overlay_creation() {
        let overlay = SignLanguageOverlay::default();
        assert!((overlay.config().opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_validation() {
        let mut overlay = SignLanguageOverlay::default();
        assert!(overlay.validate().is_ok());

        overlay.config.opacity = 1.5;
        assert!(overlay.validate().is_err());
    }

    // ── Format / shape honesty ───────────────────────────────────────────────

    #[test]
    fn apply_rejects_non_yuv420p_main_frame() {
        let overlay = SignLanguageOverlay::default();
        let main = VideoFrame::new(
            PixelFormat::Rgb24,
            32,
            32,
            timestamp(),
            vec![vec![0u8; 32 * 32 * 3]],
            vec![32 * 3],
            true,
        );
        let sign = patterned_frame(16, 16);
        let error = overlay.apply(&main, &sign).expect_err("wrong format");
        assert!(matches!(error, AccessError::SignLanguageFailed(_)));
    }

    #[test]
    fn apply_rejects_non_yuv420p_sign_frame() {
        let overlay = SignLanguageOverlay::default();
        let main = patterned_frame(64, 64);
        let sign = VideoFrame::new(
            PixelFormat::Gray8,
            16,
            16,
            timestamp(),
            vec![vec![0u8; 16 * 16]],
            vec![16],
            true,
        );
        let error = overlay.apply(&main, &sign).expect_err("wrong format");
        assert!(matches!(error, AccessError::SignLanguageFailed(_)));
    }

    #[test]
    fn apply_rejects_zero_sized_frames() {
        let overlay = SignLanguageOverlay::default();
        let main = patterned_frame(64, 64);
        let degenerate = VideoFrame::new(
            PixelFormat::Yuv420p,
            0,
            0,
            timestamp(),
            vec![vec![], vec![], vec![]],
            vec![0, 0, 0],
            true,
        );
        assert!(overlay.apply(&main, &degenerate).is_err());
        assert!(overlay.apply(&degenerate, &main).is_err());
    }

    #[test]
    fn apply_is_honest_err_not_fabricated_empty_frame() {
        // The previous behaviour (`Ok(vec![])` for any input) is gone; a
        // *valid* Yuv420p pair now really composites (see the tests below).
        // This checks the still-relevant honesty case: an invalid opacity
        // must not silently produce a frame either.
        let mut overlay = SignLanguageOverlay::default();
        overlay.set_config(config(SignPosition::BottomRight, SignSize::Small, None));
        let mut bad_config = overlay.config().clone();
        bad_config.opacity = 1.5;
        overlay.set_config(bad_config);

        let main = patterned_frame(64, 64);
        let sign = patterned_frame(16, 16);
        let result = overlay.apply(&main, &sign);

        assert!(
            result.is_err(),
            "apply() must not fabricate a composited frame from an invalid config"
        );
        assert!(matches!(
            result.unwrap_err(),
            AccessError::SignLanguageFailed(_)
        ));
    }

    // ── Real compositing ─────────────────────────────────────────────────────

    #[test]
    fn apply_exact_1to1_overlay_matches_the_source_pixel_for_pixel() {
        // `SignSize::Custom` sized so the scaled overlay is *exactly* the
        // sign frame's own resolution: no scaling, so every overlay-region
        // pixel must equal the source exactly (opacity 1.0, no border).
        let main = patterned_frame(64, 48);
        let sign = solid_frame(16, 16, 200, 90, 60);
        // 16 / 64 = 25%.
        let overlay =
            SignLanguageOverlay::new(config(SignPosition::TopLeft, SignSize::Custom(25), None));

        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let y_stride = composited.strides[0];

        // TopLeft with the 16px margin: video region starts at (16, 16),
        // sized 16x16 (25% of 64 == 16, already even).
        for row in 16..32u32 {
            for col in 16..32u32 {
                let idx = row as usize * y_stride + col as usize;
                assert_eq!(
                    composited.planes[0][idx], 200,
                    "luma mismatch at ({col},{row})"
                );
            }
        }
        let c_stride = composited.strides[1];
        for row in 8..16u32 {
            for col in 8..16u32 {
                let idx = row as usize * c_stride + col as usize;
                assert_eq!(composited.planes[1][idx], 90, "U mismatch at ({col},{row})");
                assert_eq!(composited.planes[2][idx], 60, "V mismatch at ({col},{row})");
            }
        }
        // Outside the overlay region, the main frame is untouched.
        assert_eq!(
            composited.planes[0][0], main.planes[0][0],
            "pixels outside the overlay must be untouched"
        );
    }

    #[test]
    fn apply_preserves_untouched_regions_of_a_patterned_main_frame() {
        let main = patterned_frame(80, 60);
        let sign = patterned_frame(20, 20);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::TopLeft,
            SignSize::Custom(10), // small overlay, well clear of most pixels
            None,
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");

        // The bottom-right quadrant is nowhere near a TopLeft overlay this
        // small, so it must match the original pattern exactly.
        let stride = composited.strides[0];
        for row in 40..60u32 {
            for col in 40..80u32 {
                let idx = row as usize * stride + col as usize;
                assert_eq!(
                    composited.planes[0][idx], main.planes[0][idx],
                    "untouched region must match the source pattern at ({col},{row})"
                );
            }
        }
    }

    #[test]
    fn apply_opacity_blends_rather_than_replaces() {
        let main = solid_frame(64, 64, 20, 20, 20);
        let sign = solid_frame(16, 16, 220, 220, 220);
        let overlay = SignLanguageOverlay::new(SignConfig {
            position: SignPosition::TopLeft,
            size: SignSize::Custom(25),
            border: None,
            opacity: 0.5,
        });
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];
        let idx = 20usize * stride + 20; // inside the overlay region
        let blended = composited.planes[0][idx];
        // 0.5 * 220 + 0.5 * 20 == 120, exactly representable.
        assert_eq!(blended, 120, "expected a 50/50 blend, got {blended}");
    }

    #[test]
    fn apply_solid_border_paints_the_configured_color() {
        let main = solid_frame(64, 64, 0, 128, 128);
        let sign = solid_frame(16, 16, 255, 128, 128);
        let border_color = (255u8, 0u8, 0u8, 255u8); // opaque red
        let (expected_y, expected_u, expected_v) =
            rgb_to_yuv(border_color.0, border_color.1, border_color.2);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::TopLeft,
            SignSize::Custom(25),
            Some(SignBorder::solid(4, border_color)),
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");

        // TopLeft margin 16px: the outer (border+video) box is [16,40) on
        // both axes. Border 4px inset the video box to [20,36); the ring is
        // the 4px frame between the two, e.g. row 17 (in [16,20), the top
        // strip) at any column in [16,40).
        let stride = composited.strides[0];
        let idx = 17usize * stride + 25; // inside the top border strip
        assert_eq!(composited.planes[0][idx], expected_y, "border Y mismatch");

        // Corresponding chroma sample: luma (24, 16) -> chroma (12, 8),
        // still inside the top strip (chroma row 8 -> luma row 16 < 20).
        let c_stride = composited.strides[1];
        let c_idx = 8usize * c_stride + 12;
        assert_eq!(composited.planes[1][c_idx], expected_u, "border U mismatch");
        assert_eq!(composited.planes[2][c_idx], expected_v, "border V mismatch");
    }

    #[test]
    fn apply_border_style_none_paints_no_ring() {
        let main = solid_frame(64, 64, 10, 10, 10);
        let sign = solid_frame(16, 16, 200, 200, 200);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::TopLeft,
            SignSize::Custom(25),
            Some(SignBorder {
                style: SignBorderStyle::None,
                width: 4,
                color: (255, 0, 0, 255),
                radius: 0,
            }),
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];
        // With no border, the video box sits flush at the margin (16,16);
        // the pixel just outside it must be untouched main-frame content.
        let idx = 14usize * stride + 20;
        assert_eq!(composited.planes[0][idx], 10, "no border should be drawn");
    }

    #[test]
    fn apply_rounded_border_leaves_the_far_corner_pixel_untouched() {
        let main = solid_frame(64, 64, 10, 10, 10);
        let sign = solid_frame(16, 16, 200, 200, 200);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::TopLeft,
            SignSize::Custom(25),
            Some(SignBorder::rounded(4, (255, 0, 0, 255), 8)),
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];
        // Outer (border+video) box top-left is (16, 16) (TopLeft margin
        // 16px). The exact corner pixel (16,16) is `radius` away from the
        // arc center (16+8, 16+8) = (24,24) by sqrt(8^2+8^2) ≈ 11.3 >
        // radius(8), so it must be cut — left as the untouched main value.
        let idx = 16usize * stride + 16;
        assert_eq!(
            composited.planes[0][idx], 10,
            "rounded corner must leave the extreme corner pixel untouched"
        );
    }

    // ── Chroma / even-rounding alignment (observable through output) ────────

    #[test]
    fn apply_custom_position_rounds_the_overlay_left_edge_to_even() {
        let main = solid_frame(100, 100, 5, 5, 5);
        let sign = solid_frame(20, 20, 250, 250, 250);
        // free_w = 100 - 20 = 80; raw_x = 80 * 7 / 100 = 5 (truncated),
        // which is odd -- round_down_even must bring it down to 4.
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::Custom(7, 0),
            SignSize::Custom(20),
            None,
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];
        let row = 0usize; // top row: y offset is 0 regardless (y_pct = 0)

        let first_overlay_col = (0..100u32)
            .find(|&col| composited.planes[0][row * stride + col as usize] == 250)
            .expect("overlay must paint somewhere in the top row");
        assert_eq!(
            first_overlay_col, 4,
            "raw offset 5 must round down to the even column 4"
        );
    }

    #[test]
    fn apply_custom_position_rounds_the_overlay_top_edge_to_even() {
        let main = solid_frame(100, 100, 5, 5, 5);
        let sign = solid_frame(20, 20, 250, 250, 250);
        // Symmetric with the left-edge case above: free_h = 80, raw_y =
        // 80 * 7 / 100 = 5 (odd) -> rounds down to 4.
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::Custom(0, 7),
            SignSize::Custom(20),
            None,
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];
        let col = 0usize;

        let first_overlay_row = (0..100u32)
            .find(|&row| composited.planes[0][row as usize * stride + col] == 250)
            .expect("overlay must paint somewhere in the left column");
        assert_eq!(
            first_overlay_row, 4,
            "raw offset 5 must round down to the even row 4"
        );
    }

    #[test]
    fn apply_chroma_planes_show_no_color_shift_at_the_overlay_boundary() {
        // A patterned main frame plus a solid overlay: every chroma sample
        // fully inside the overlay region must equal the overlay's chroma
        // exactly (not a blend with the neighbouring pattern), which would
        // not hold if the overlay's chroma-plane offset were off by one
        // sample from its luma-plane offset.
        let main = patterned_frame(64, 64);
        let sign = solid_frame(16, 16, 128, 40, 220);
        let overlay =
            SignLanguageOverlay::new(config(SignPosition::TopLeft, SignSize::Custom(25), None));
        let composited = overlay.apply(&main, &sign).expect("valid composite");

        let c_stride = composited.strides[1];
        // Video box (25% of 64 = 16, margin 16): luma (16..32, 16..32) ->
        // chroma (8..16, 8..16).
        for row in 8..16u32 {
            for col in 8..16u32 {
                let idx = row as usize * c_stride + col as usize;
                assert_eq!(composited.planes[1][idx], 40, "U shifted at ({col},{row})");
                assert_eq!(composited.planes[2][idx], 220, "V shifted at ({col},{row})");
            }
        }
    }

    #[test]
    fn apply_scales_a_non_square_sign_frame_preserving_source_pattern_shape() {
        // Not an exact 1:1 case: the sign frame is smaller than the target
        // region, so nearest-neighbor scaling is exercised. Every sample
        // read back from the overlay region must be *some* value the
        // source actually contains (never fabricated), and corners must
        // map to the source's own corners.
        let sign = patterned_frame(8, 8);
        let main = solid_frame(64, 64, 1, 1, 1);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::TopLeft,
            SignSize::Custom(25), // 16x16 target from an 8x8 source: 2x scale
            None,
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];

        // Top-left of the video box (16,16) maps to source (0,0).
        let top_left = composited.planes[0][16usize * stride + 16];
        assert_eq!(top_left, sign.planes[0][0]);

        let valid_values: std::collections::HashSet<u8> = sign.planes[0].iter().copied().collect();
        for row in 16..32u32 {
            for col in 16..32u32 {
                let value = composited.planes[0][row as usize * stride + col as usize];
                assert!(
                    valid_values.contains(&value),
                    "scaled pixel ({col},{row}) = {value} is not one of the source's own values"
                );
            }
        }
    }

    #[test]
    fn apply_downscales_a_larger_sign_frame_preserving_source_pattern_shape() {
        // The common real-world direction: the sign-language source is
        // *bigger* than the region it is composited into (e.g. a 1280x720
        // interpreter feed shrunk into a small PiP box) — the previous test
        // only exercised upscaling. Every sample read back from the overlay
        // region must still be one of the source's own values.
        let sign = patterned_frame(32, 32);
        let main = solid_frame(64, 64, 1, 1, 1);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::TopLeft,
            SignSize::Custom(25), // 16x16 target from a 32x32 source: 0.5x scale
            None,
        ));
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        let stride = composited.strides[0];

        let top_left = composited.planes[0][16usize * stride + 16];
        assert_eq!(top_left, sign.planes[0][0]);

        let valid_values: std::collections::HashSet<u8> = sign.planes[0].iter().copied().collect();
        for row in 16..32u32 {
            for col in 16..32u32 {
                let value = composited.planes[0][row as usize * stride + col as usize];
                assert!(
                    valid_values.contains(&value),
                    "downscaled pixel ({col},{row}) = {value} is not one of the source's own \
                     values"
                );
            }
        }
    }

    #[test]
    fn apply_shrinks_an_oversized_overlay_to_fit_the_main_frame() {
        // Sign frame requests, via a large `Custom` percent, an overlay
        // bigger than the main frame could ever host with any border and
        // margin left over; this must clamp rather than error or panic.
        let main = solid_frame(32, 32, 9, 9, 9);
        let sign = solid_frame(32, 32, 250, 250, 250);
        let overlay = SignLanguageOverlay::new(config(
            SignPosition::BottomRight,
            SignSize::Custom(100),
            Some(SignBorder::solid(6, (0, 255, 0, 255))),
        ));
        let composited = overlay.apply(&main, &sign).expect("must clamp, not fail");
        assert_eq!(composited.width, 32);
        assert_eq!(composited.height, 32);
    }

    #[test]
    fn apply_zero_opacity_leaves_the_main_frame_untouched() {
        let main = solid_frame(32, 32, 77, 77, 77);
        let sign = solid_frame(16, 16, 5, 5, 5);
        let overlay = SignLanguageOverlay::new(SignConfig {
            position: SignPosition::TopLeft,
            size: SignSize::Custom(50),
            border: Some(SignBorder::solid(2, (255, 0, 0, 255))),
            opacity: 0.0,
        });
        let composited = overlay.apply(&main, &sign).expect("valid composite");
        assert_eq!(
            composited.planes, main.planes,
            "zero opacity must paint nothing"
        );
    }
}
