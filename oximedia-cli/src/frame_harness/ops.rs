//! Whole-clip and per-frame operations built on the frame harness.
//!
//! Each operation here is real work on real pixels. Operations that need
//! global context across the clip (stabilisation derives one motion trajectory
//! from every frame) take a [`PlanarClip`]; streaming operations take a single
//! [`super::PlanarFrame`].

use anyhow::Result;

use super::{ChromaLayout, PlanarClip, PlanarFrame};

/// Stabilise a whole clip with the `oximedia-stabilize` offline multi-pass
/// pipeline.
///
/// # Pipeline
///
/// 1. Extract the luma (Y) plane of every frame and hand it to the
///    `oximedia-stabilize` offline multi-pass stabiliser, which performs
///    multi-pass analysis, feature-based motion estimation, trajectory
///    smoothing and frame warping.
/// 2. When `config.enable_zoom_optimization` is set, scale the transforms
///    with a real [`oximedia_stabilize::zoom::calculate::ZoomOptimizer`] —
///    mirroring step 8 of `Stabilizer::stabilize` — to hide the black
///    borders stabilisation warping introduces.
/// 3. Apply the *same* per-frame stabilisation transforms to the chroma
///    planes (translation scaled for chroma subsampling) so colour tracks the
///    luma, and to the alpha plane at full resolution.
///
/// `config` carries the caller's motion model, quality preset, smoothing
/// strength and zoom-optimisation choice (`oximedia stabilize`'s
/// `--mode`/`--quality`/`--smoothing`/`--zoom` flags map directly onto it);
/// this function no longer hard-codes those. Geometry is preserved exactly,
/// so the result can be muxed back into the source clip's Y4M header.
///
/// # Errors
///
/// Returns an error if `config` is invalid, if a frame's size disagrees with
/// the clip layout, or if any stage of the stabilisation pipeline fails.
pub fn stabilize_clip(
    clip: &PlanarClip,
    config: &oximedia_stabilize::StabilizeConfig,
) -> Result<PlanarClip> {
    use oximedia_stabilize::motion::estimate::MotionEstimator;
    use oximedia_stabilize::motion::tracker::MotionTracker;
    use oximedia_stabilize::motion::trajectory::Trajectory;
    use oximedia_stabilize::multipass::analyze::MultipassAnalyzer;
    use oximedia_stabilize::smooth::filter::TrajectorySmoother;
    use oximedia_stabilize::transform::calculate::{StabilizationTransform, TransformCalculator};
    use oximedia_stabilize::warp::apply::FrameWarper;
    use oximedia_stabilize::zoom::calculate::ZoomOptimizer;
    use oximedia_stabilize::Frame;
    use scirs2_core::ndarray::Array2;

    config
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid stabilisation configuration: {e}"))?;

    let layout = clip.layout;
    let expected = layout.frame_size();
    for (i, f) in clip.frames.iter().enumerate() {
        if f.data.len() != expected {
            anyhow::bail!(
                "Y4M frame {i} has {} bytes, expected {expected} for the declared geometry",
                f.data.len()
            );
        }
    }

    // ----- Build oximedia-stabilize luma frames --------------------------
    let fps = f64::from(clip.header.fps_num.max(1)) / f64::from(clip.header.fps_den.max(1));
    let luma_frames: Vec<Frame> = clip
        .frames
        .iter()
        .enumerate()
        .map(|(i, frame)| -> Result<Frame> {
            let data =
                Array2::from_shape_vec((layout.luma_h, layout.luma_w), frame.luma().to_vec())
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to build luma array for frame {i}: {e}")
                    })?;
            Ok(Frame::new(
                layout.luma_w,
                layout.luma_h,
                i as f64 / fps,
                data,
            ))
        })
        .collect::<Result<_>>()?;

    // ----- Offline multi-pass stabilisation pipeline ---------------------
    // This mirrors `oximedia_stabilize::Stabilizer::stabilize`, but keeps the
    // intermediate per-frame transforms so they can be re-applied to the
    // chroma planes.

    // Pass 1 — analyse the whole clip up front (multi-pass / offline).
    let analyzer = MultipassAnalyzer::new();
    let analysis = analyzer
        .analyze(&luma_frames)
        .map_err(|e| anyhow::anyhow!("Multi-pass analysis failed: {e}"))?;
    // Use the analysis to pick the smoothing window, exactly as the offline
    // stabiliser does when adapting to the detected motion profile.
    let smoothing_window = analysis
        .recommended_window_size
        .max(config.quality.smoothing_window())
        .max(1);

    // Pass 2 — feature tracking + motion estimation + smoothing + warp.
    let transforms: Vec<StabilizationTransform> = {
        let mut tracker = MotionTracker::new(config.feature_count);
        match tracker.track(&luma_frames) {
            Ok(tracks) => {
                let estimator = MotionEstimator::new(config.mode);
                let models = estimator
                    .estimate(&tracks, luma_frames.len())
                    .map_err(|e| anyhow::anyhow!("Motion estimation failed: {e}"))?;
                let trajectory = Trajectory::from_models(&models)
                    .map_err(|e| anyhow::anyhow!("Trajectory build failed: {e}"))?;
                let mut smoother =
                    TrajectorySmoother::new(smoothing_window, config.smoothing_strength);
                let smoothed = smoother
                    .smooth(&trajectory)
                    .map_err(|e| anyhow::anyhow!("Trajectory smoothing failed: {e}"))?;
                let calculator = TransformCalculator::new();
                let calculated = calculator
                    .calculate(&trajectory, &smoothed)
                    .map_err(|e| anyhow::anyhow!("Transform calculation failed: {e}"))?;

                // Step 8 of `Stabilizer::stabilize`: minimise the black
                // borders the warp introduces by scaling the transforms up.
                // Only reached with real (non-identity) transforms, matching
                // upstream — the `InsufficientFeatures` fallback below never
                // ran a zoom pass either (identity transforms have zero
                // magnitude, so zoom would be a no-op regardless).
                if config.enable_zoom_optimization {
                    let zoom_optimizer = ZoomOptimizer::new(config.crop_ratio);
                    zoom_optimizer
                        .optimize(&calculated, layout.luma_w, layout.luma_h)
                        .map_err(|e| anyhow::anyhow!("Zoom optimisation failed: {e}"))?
                } else {
                    calculated
                }
            }
            Err(oximedia_stabilize::StabilizeError::InsufficientFeatures { .. }) => {
                // Featureless footage (e.g. flat colour): nothing to correct,
                // fall back to identity transforms so the clip passes through.
                (0..luma_frames.len())
                    .map(StabilizationTransform::identity)
                    .collect()
            }
            Err(e) => return Err(anyhow::anyhow!("Motion tracking failed: {e}")),
        }
    };

    if transforms.len() != clip.frames.len() {
        anyhow::bail!(
            "stabiliser produced {} transforms for {} frames",
            transforms.len(),
            clip.frames.len()
        );
    }

    // ----- Warp every plane with the per-frame transforms ----------------
    let warper = FrameWarper::new();
    let mut out = clip.empty_like();
    out.frames.reserve(clip.frames.len());

    for (frame, transform) in clip.frames.iter().zip(transforms.iter()) {
        let mut dst = vec![0u8; expected];
        let mut offset = 0usize;

        // Luma plane — full-resolution transform.
        let luma_len = layout.luma_len();
        warp_plane(
            &warper,
            &frame.data[offset..offset + luma_len],
            layout.luma_w,
            layout.luma_h,
            transform,
            1.0,
            &mut dst[offset..offset + luma_len],
        )?;
        offset += luma_len;

        // Chroma planes — translation scaled by the subsampling ratio.
        if layout.chroma_w > 0 && layout.chroma_h > 0 {
            let plane_len = layout.chroma_len();
            let scale_x = layout.chroma_w as f64 / layout.luma_w.max(1) as f64;
            let scale_y = layout.chroma_h as f64 / layout.luma_h.max(1) as f64;
            // Average ratio keeps the helper's single-scale model simple while
            // remaining exact for 4:2:0 / 4:2:2 / 4:4:4 (uniform per axis).
            let chroma_scale = (scale_x + scale_y) / 2.0;
            for _ in 0..2 {
                warp_plane(
                    &warper,
                    &frame.data[offset..offset + plane_len],
                    layout.chroma_w,
                    layout.chroma_h,
                    transform,
                    chroma_scale,
                    &mut dst[offset..offset + plane_len],
                )?;
                offset += plane_len;
            }
        }

        // Alpha plane (C444alpha) — full-resolution, same transform as luma.
        if layout.has_alpha {
            warp_plane(
                &warper,
                &frame.data[offset..offset + luma_len],
                layout.luma_w,
                layout.luma_h,
                transform,
                1.0,
                &mut dst[offset..offset + luma_len],
            )?;
        }

        out.frames.push(PlanarFrame::new(layout, dst)?);
    }

    Ok(out)
}

/// Warp a single 8-bit plane with one stabilisation transform.
///
/// `translation_scale` rescales the transform's translation component so that
/// a luma-derived transform can be applied to a subsampled chroma plane.
fn warp_plane(
    warper: &oximedia_stabilize::warp::apply::FrameWarper,
    src: &[u8],
    plane_w: usize,
    plane_h: usize,
    transform: &oximedia_stabilize::transform::calculate::StabilizationTransform,
    translation_scale: f64,
    dst: &mut [u8],
) -> Result<()> {
    use oximedia_stabilize::transform::calculate::StabilizationTransform;
    use oximedia_stabilize::Frame;
    use scirs2_core::ndarray::Array2;

    if plane_w == 0 || plane_h == 0 {
        return Ok(());
    }

    let data = Array2::from_shape_vec((plane_h, plane_w), src.to_vec())
        .map_err(|e| anyhow::anyhow!("Failed to build plane array ({plane_w}x{plane_h}): {e}"))?;
    // The warper copies `timestamp` straight through and does not use it in
    // the warp math, so any value is fine for this throwaway single-frame call.
    let frame = Frame::new(plane_w, plane_h, 0.0, data);

    // Rotation and scale are dimensionless and apply unchanged at any
    // resolution; only the translation must be rescaled for chroma planes.
    let plane_transform = StabilizationTransform {
        dx: transform.dx * translation_scale,
        dy: transform.dy * translation_scale,
        angle: transform.angle,
        scale: transform.scale,
        frame_index: transform.frame_index,
        confidence: transform.confidence,
    };

    let warped = warper
        .warp(
            std::slice::from_ref(&frame),
            std::slice::from_ref(&plane_transform),
        )
        .map_err(|e| anyhow::anyhow!("Frame warp failed: {e}"))?;
    let warped_frame = warped
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Frame warp returned no frame"))?;

    // Copy the warped plane back out in row-major order.
    let warped_bytes: Vec<u8> = warped_frame.data.iter().copied().collect();
    if warped_bytes.len() != dst.len() {
        anyhow::bail!(
            "warped plane has {} bytes, expected {}",
            warped_bytes.len(),
            dst.len()
        );
    }
    dst.copy_from_slice(&warped_bytes);
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-frame statistics
// ---------------------------------------------------------------------------

/// Mean and standard deviation of a clip's RGB channels, plus the sample
/// count the statistics were averaged over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbStats {
    /// Per-channel mean, normalised to `0.0..=1.0`.
    pub mean_rgb: [f32; 3],
    /// Per-channel population standard deviation, in the same units.
    pub std_rgb: [f32; 3],
    /// Number of luma-resolution pixels the statistics were computed over.
    pub pixel_count: u64,
    /// Number of frames sampled.
    pub frame_count: usize,
}

/// Compute per-channel RGB mean/stddev over every pixel of every frame.
///
/// Each luma sample is paired with the chroma samples covering it (nearest
/// co-sited sample for subsampled formats) and converted with
/// [`oximedia_core::convert::pixel::PixelConverter`], i.e. limited-range
/// BT.709 YCbCr → gamma-encoded 8-bit sRGB. The returned means and standard
/// deviations are therefore statistics of **gamma-encoded** channel values,
/// scaled to `0.0..=1.0`.
///
/// Monochrome clips are treated as neutral grey (Cb = Cr = 128), which is what
/// the Y4M `Mono` tag means.
///
/// # Errors
///
/// Returns an error if the clip has no frames.
pub fn clip_rgb_stats(clip: &PlanarClip) -> Result<RgbStats> {
    use oximedia_core::convert::pixel::{ColorMatrix, PixelConverter};

    if clip.frames.is_empty() {
        anyhow::bail!("cannot compute colour statistics for a clip with no frames");
    }

    let layout: ChromaLayout = clip.layout;
    let converter = PixelConverter::new(ColorMatrix::Bt709);

    let mut sum = [0f64; 3];
    let mut sum_sq = [0f64; 3];
    let mut count: u64 = 0;

    for frame in &clip.frames {
        let luma = frame.luma();
        let (cb, cr) = match (frame.plane(1), frame.plane(2)) {
            (Some(u), Some(v)) => (Some(u), Some(v)),
            _ => (None, None),
        };

        // Integer subsampling ratios; 1 when the plane is absent.
        let h_ratio = if layout.chroma_w > 0 {
            layout.luma_w.div_ceil(layout.chroma_w).max(1)
        } else {
            1
        };
        let v_ratio = if layout.chroma_h > 0 {
            layout.luma_h.div_ceil(layout.chroma_h).max(1)
        } else {
            1
        };

        for y in 0..layout.luma_h {
            for x in 0..layout.luma_w {
                let y_val = luma[y * layout.luma_w + x];
                let (u_val, v_val) = match (cb, cr) {
                    (Some(u), Some(v)) => {
                        let cx = (x / h_ratio).min(layout.chroma_w.saturating_sub(1));
                        let cy = (y / v_ratio).min(layout.chroma_h.saturating_sub(1));
                        let idx = cy * layout.chroma_w + cx;
                        (u[idx], v[idx])
                    }
                    _ => (128, 128),
                };
                let (r, g, b) = converter.yuv_to_rgb(y_val, u_val, v_val);
                for (i, channel) in [r, g, b].into_iter().enumerate() {
                    let value = f64::from(channel) / 255.0;
                    sum[i] += value;
                    sum_sq[i] += value * value;
                }
                count += 1;
            }
        }
    }

    if count == 0 {
        anyhow::bail!("clip has zero pixels; cannot compute colour statistics");
    }

    let n = count as f64;
    let mut mean_rgb = [0f32; 3];
    let mut std_rgb = [0f32; 3];
    for (((mean_out, std_out), &total), &total_sq) in mean_rgb
        .iter_mut()
        .zip(std_rgb.iter_mut())
        .zip(sum.iter())
        .zip(sum_sq.iter())
    {
        let mean = total / n;
        let variance = (total_sq / n - mean * mean).max(0.0);
        *mean_out = mean as f32;
        *std_out = variance.sqrt() as f32;
    }

    Ok(RgbStats {
        mean_rgb,
        std_rgb,
        pixel_count: count,
        frame_count: clip.frames.len(),
    })
}

/// Lowest CCT McCamy's cubic is treated as meaningful at.
const CCT_MIN_KELVIN: f64 = 1_000.0;
/// Highest CCT McCamy's cubic is treated as meaningful at.
const CCT_MAX_KELVIN: f64 = 25_000.0;

/// Correlated colour temperature (Kelvin) from a mean sRGB triple, via
/// McCamy's cubic approximation — or `None` when the approximation does not
/// apply.
///
/// The input is linearised from the gamma-encoded sRGB means produced by
/// [`clip_rgb_stats`], converted to CIE 1931 XYZ with the sRGB/BT.709
/// primaries and D65 white, then reduced to chromaticity `(x, y)`.
///
/// # Why this returns `Option`
///
/// McCamy's cubic is fitted near the Planckian locus and diverges away from
/// it: a saturated blue frame drives it to large negative values, and a
/// saturated red to implausibly small ones. Clamping those into a physical
/// range would report a confident-looking temperature that measures nothing,
/// so out-of-range results are reported as "no estimate" instead.
///
/// The gate is the quoted output range (`1000..=25000 K`), not a rigorous
/// proximity test against the locus: it reliably rejects chromaticities where
/// the cubic has clearly broken down, but it is not a Duv computation and a
/// strongly off-locus colour can still land inside the window. Treat the
/// number as an indicative white-balance reading for near-neutral footage.
#[must_use]
pub fn correlated_color_temperature(mean_rgb: [f32; 3]) -> Option<f32> {
    let (cx, cy) = chromaticity(mean_rgb)?;

    // McCamy's approximation: CCT = 449 n^3 + 3525 n^2 + 6823.3 n + 5520.33
    // with n = (x - 0.3320) / (0.1858 - y).
    let denominator = 0.1858 - cy;
    if denominator.abs() < 1e-9 {
        return None;
    }
    let n = (cx - 0.3320) / denominator;
    let cct = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
    if !cct.is_finite() || !(CCT_MIN_KELVIN..=CCT_MAX_KELVIN).contains(&cct) {
        return None;
    }
    Some(cct as f32)
}

/// Linearise a gamma-encoded sRGB channel value.
fn srgb_to_linear(channel: f32) -> f64 {
    let c = f64::from(channel.clamp(0.0, 1.0));
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// CIE 1931 `(x, y)` chromaticity of a gamma-encoded sRGB triple.
///
/// Returns `None` for pure black, which has no chromaticity.
fn chromaticity(mean_rgb: [f32; 3]) -> Option<(f64, f64)> {
    let r = srgb_to_linear(mean_rgb[0]);
    let g = srgb_to_linear(mean_rgb[1]);
    let b = srgb_to_linear(mean_rgb[2]);

    // sRGB (BT.709 primaries, D65) → CIE 1931 XYZ.
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;

    let sum = x + y + z;
    if sum <= 0.0 {
        return None;
    }
    Some((x / sum, y / sum))
}

/// Green–magenta balance of a mean sRGB triple, in linear light.
///
/// Defined as `G - (R + B) / 2` over the linearised channel means: positive
/// values are green-biased, negative values magenta-biased, `0.0` neutral.
/// This is a plain measured statistic, not a camera-vendor tint scale — and
/// unlike the CCT estimate it is always defined.
#[must_use]
pub fn green_magenta_tint(mean_rgb: [f32; 3]) -> f32 {
    let r = srgb_to_linear(mean_rgb[0]);
    let g = srgb_to_linear(mean_rgb[1]);
    let b = srgb_to_linear(mean_rgb[2]);
    (g - (r + b) / 2.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_harness::PlanarClip;
    use oximedia_container::demux::y4m::{Y4mChroma, Y4mHeader, Y4mInterlace};

    fn header(width: u32, height: u32) -> Y4mHeader {
        Y4mHeader {
            width,
            height,
            fps_num: 25,
            fps_den: 1,
            interlace: Y4mInterlace::Progressive,
            par_num: 1,
            par_den: 1,
            chroma: Y4mChroma::C420jpeg,
            comment: None,
        }
    }

    /// Build a flat 4:2:0 clip with the given Y/Cb/Cr values.
    fn flat_clip(width: u32, height: u32, frames: usize, y: u8, u: u8, v: u8) -> PlanarClip {
        let h = header(width, height);
        let w = width as usize;
        let hh = height as usize;
        let cw = w.div_ceil(2);
        let ch = hh.div_ceil(2);
        let mut raw = Vec::new();
        for _ in 0..frames {
            let mut buf = vec![y; w * hh];
            buf.extend(std::iter::repeat_n(u, cw * ch));
            buf.extend(std::iter::repeat_n(v, cw * ch));
            raw.push(buf);
        }
        PlanarClip::from_raw_frames(h, raw).expect("clip")
    }

    /// A checkerboard pattern with jittery (non-monotonic) per-frame shifts.
    ///
    /// Confirmed empirically (`MotionTracker::track` on this exact pattern
    /// reliably yields 100+ feature tracks) to give the Harris-corner tracker
    /// real, trackable features — unlike the flat clip above, which always
    /// falls back to identity transforms. The jitter (as opposed to a smooth
    /// linear pan, which a smoother would mostly reproduce as-is, leaving a
    /// near-zero correction) is what gives the stabiliser a non-trivial
    /// correction to compute, which is what `--zoom`'s output depends on.
    fn checkerboard_clip(width: u32, height: u32, shifts: &[i64]) -> PlanarClip {
        let h = header(width, height);
        let w = width as usize;
        let hh = height as usize;
        let cw = w.div_ceil(2);
        let ch = hh.div_ceil(2);
        let mut cumulative = 0i64;
        let mut raw = Vec::new();
        for &shift in shifts {
            cumulative += shift;
            let mut buf = Vec::with_capacity(w * hh + 2 * cw * ch);
            for y in 0..hh {
                for x in 0..w {
                    let sx = (x as i64 - cumulative).rem_euclid(w as i64) as usize;
                    let sy = (y as i64 - cumulative).rem_euclid(hh as i64) as usize;
                    let cell = (sx / 8 + sy / 8) % 2;
                    buf.push(if cell == 0 { 30u8 } else { 220u8 });
                }
            }
            buf.extend(std::iter::repeat_n(128u8, cw * ch * 2));
            raw.push(buf);
        }
        PlanarClip::from_raw_frames(h, raw).expect("clip")
    }

    /// `--zoom` is only observable on footage with real, trackable, jittery
    /// motion (see [`checkerboard_clip`]): on flat footage the pipeline takes
    /// the `InsufficientFeatures` fallback and never reaches the zoom step at
    /// all (covered separately below), and on a perfectly smooth pan the
    /// stabiliser's own correction is close to zero regardless of `--zoom`.
    #[test]
    fn zoom_optimization_changes_the_warp_on_trackable_jittery_motion() {
        let clip = checkerboard_clip(80, 60, &[0, 6, -6, 6, -6, 6]);

        let base = oximedia_stabilize::StabilizeConfig::new()
            .with_mode(oximedia_stabilize::StabilizationMode::Affine)
            .with_quality(oximedia_stabilize::QualityPreset::Balanced)
            .with_smoothing_strength(0.85);

        let without_zoom = stabilize_clip(&clip, &base.clone().with_zoom_optimization(false))
            .expect("stabilise without zoom");
        let with_zoom =
            stabilize_clip(&clip, &base.with_zoom_optimization(true)).expect("stabilise with zoom");

        assert_eq!(without_zoom.frames.len(), with_zoom.frames.len());
        assert_eq!(without_zoom.layout, clip.layout);
        assert_eq!(with_zoom.layout, clip.layout);

        let differs = without_zoom
            .frames
            .iter()
            .zip(with_zoom.frames.iter())
            .any(|(a, b)| a.data != b.data);
        assert!(
            differs,
            "--zoom must change the warped output on footage with real, trackable, jittery motion"
        );
    }

    /// On flat (featureless) footage, `--zoom` must not be able to turn a
    /// no-correction identity pass into a spurious edit: the pipeline falls
    /// back to identity transforms before the zoom step is ever reached, so
    /// `--zoom` true/false must produce byte-identical output here.
    #[test]
    fn zoom_optimization_is_a_noop_on_featureless_footage() {
        let clip = flat_clip(32, 24, 5, 128, 128, 128);
        let base = oximedia_stabilize::StabilizeConfig::new()
            .with_mode(oximedia_stabilize::StabilizationMode::Affine)
            .with_quality(oximedia_stabilize::QualityPreset::Balanced)
            .with_smoothing_strength(0.85);

        let without_zoom = stabilize_clip(&clip, &base.clone().with_zoom_optimization(false))
            .expect("stabilise without zoom");
        let with_zoom =
            stabilize_clip(&clip, &base.with_zoom_optimization(true)).expect("stabilise with zoom");

        for (a, b) in without_zoom.frames.iter().zip(with_zoom.frames.iter()) {
            assert_eq!(
                a.data, b.data,
                "flat footage takes the InsufficientFeatures fallback before the zoom step"
            );
        }
    }

    #[test]
    fn neutral_grey_has_equal_channel_means_and_zero_spread() {
        // Y=128, Cb=Cr=128 is a neutral mid grey.
        let clip = flat_clip(16, 16, 2, 128, 128, 128);
        let stats = clip_rgb_stats(&clip).expect("stats");
        assert_eq!(stats.pixel_count, 16 * 16 * 2);
        assert_eq!(stats.frame_count, 2);
        for i in 0..3 {
            assert!(
                (stats.mean_rgb[i] - stats.mean_rgb[0]).abs() < 1e-6,
                "neutral grey must have equal channel means: {:?}",
                stats.mean_rgb
            );
            assert!(
                stats.std_rgb[i] < 1e-6,
                "a flat clip must have zero spread: {:?}",
                stats.std_rgb
            );
        }
    }

    #[test]
    fn brighter_clip_has_larger_mean() {
        let dark = clip_rgb_stats(&flat_clip(8, 8, 1, 60, 128, 128)).expect("dark stats");
        let bright = clip_rgb_stats(&flat_clip(8, 8, 1, 200, 128, 128)).expect("bright stats");
        assert!(
            bright.mean_rgb[0] > dark.mean_rgb[0],
            "brighter luma must raise the mean: {:?} vs {:?}",
            bright.mean_rgb,
            dark.mean_rgb
        );
    }

    /// Near-neutral footage: a slight blue cast must read as a higher CCT than
    /// a slight red cast, and both must be physically plausible.
    #[test]
    fn mild_blue_cast_reads_cooler_than_mild_red_cast() {
        let blue = clip_rgb_stats(&flat_clip(8, 8, 1, 128, 140, 120)).expect("blue stats");
        let red = clip_rgb_stats(&flat_clip(8, 8, 1, 128, 116, 136)).expect("red stats");
        let blue_cct = correlated_color_temperature(blue.mean_rgb).expect("blue CCT");
        let red_cct = correlated_color_temperature(red.mean_rgb).expect("red CCT");
        assert!(
            blue_cct > red_cct,
            "a blue-biased frame must read cooler (higher CCT) than a red-biased one: \
             {blue_cct} vs {red_cct}"
        );
        for cct in [blue_cct, red_cct] {
            assert!(
                (1_000.0..=25_000.0).contains(&cct),
                "a near-neutral frame must produce a plausible CCT, got {cct}"
            );
        }
    }

    /// Saturated colours are far off the Planckian locus, where McCamy's cubic
    /// is meaningless. The estimate must be withheld, not clamped into a
    /// confident-looking fake.
    #[test]
    fn saturated_colors_have_no_temperature_estimate() {
        let blue = clip_rgb_stats(&flat_clip(8, 8, 1, 128, 240, 60)).expect("blue stats");
        assert_eq!(
            correlated_color_temperature(blue.mean_rgb),
            None,
            "a saturated blue must not report a colour temperature"
        );
        // The tint statistic is always defined, and must read magenta-negative
        // for a strongly blue frame (blue lifts R+B against G).
        assert!(green_magenta_tint(blue.mean_rgb) < 0.0);
    }

    #[test]
    fn tint_sign_follows_green_bias() {
        // Low Cb and low Cr both push green up in BT.709.
        let green = clip_rgb_stats(&flat_clip(8, 8, 1, 128, 90, 90)).expect("green stats");
        let magenta = clip_rgb_stats(&flat_clip(8, 8, 1, 128, 170, 170)).expect("magenta stats");
        assert!(
            green_magenta_tint(green.mean_rgb) > green_magenta_tint(magenta.mean_rgb),
            "green-biased frames must have a larger tint value"
        );
    }

    #[test]
    fn stabilize_preserves_geometry_and_frame_count() {
        // Flat footage has no trackable features: the pipeline must fall back
        // to identity transforms rather than failing.
        let clip = flat_clip(32, 24, 5, 128, 128, 128);
        let config = oximedia_stabilize::StabilizeConfig::new()
            .with_mode(oximedia_stabilize::StabilizationMode::Affine)
            .with_quality(oximedia_stabilize::QualityPreset::Balanced)
            .with_smoothing_strength(0.85);
        let out = stabilize_clip(&clip, &config).expect("stabilise flat clip");
        assert_eq!(out.frames.len(), 5);
        assert_eq!(out.layout, clip.layout);
        for frame in &out.frames {
            assert_eq!(frame.data.len(), clip.layout.frame_size());
        }
    }
}
