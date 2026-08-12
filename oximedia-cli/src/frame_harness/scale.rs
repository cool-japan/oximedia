//! Whole-clip resize: real per-plane resampling via `oximedia_scaling`, with
//! honest stretch/letterbox/crop aspect-ratio handling.
//!
//! # Why this bypasses `process_clip` / `process_frames`
//!
//! Every other operation built on this harness preserves frame geometry —
//! [`super::process_clip`] and [`super::process_frames`] both *enforce* it,
//! rejecting an operation whose output disagrees with the input's `width`,
//! `height` or plane layout (see their `Errors` docs). Resizing is the one
//! legitimate exception: producing a `--width`/`--height` output is the
//! entire point. So this module reads and writes Y4M directly with
//! [`super::read_y4m_clip_from_bytes`] / [`super::write_y4m_clip`] instead of
//! going through either geometry-preserving entry point.
//!
//! # Aspect-ratio modes
//!
//! `oximedia_scaling::VideoScaler::calculate_dimensions` computes a single
//! "cover" size (scale up until the frame fully covers the target on both
//! axes) for *both* `AspectRatioMode::Letterbox` and `AspectRatioMode::Crop`
//! — confirmed by that crate's own `test_calculate_dimensions_letterbox`,
//! which asserts a cover-sized (not contain-sized) result. Cover is exactly
//! what `Crop` needs (scale up, then crop the overflow), but `Letterbox`
//! needs the opposite: the "contain" size (scale down until the frame fits
//! entirely within the target, then pad the shortfall). `content_dimensions`
//! computes both, and `pad_to` / `crop_to` finish the job with
//! `oximedia_scaling::pad`/`crop` for the geometry and a local blit for the
//! actual pixel copy (neither module in `oximedia-scaling` moves pixels).

use std::path::Path;

use anyhow::{Context, Result};
use oximedia_container::demux::y4m::Y4mChroma;
use oximedia_scaling::crop::CropRect;
use oximedia_scaling::pad::PadOperation;
use oximedia_scaling::resampler::{FilterKernel, Resampler, ResamplerConfig};
use oximedia_scaling::AspectRatioMode;

use super::{ChromaLayout, PlanarClip, PlanarFrame};

/// Full-range black in Y/Cb/Cr, used to fill letterbox/pillarbox bars.
const PAD_LUMA: u8 = 0;
/// Neutral chroma (no colour cast) for the same bars.
const PAD_CHROMA: u8 = 128;

/// Statistics for one completed resize.
#[derive(Debug, Clone, Copy)]
pub struct ScaleStats {
    /// Number of frames written.
    pub frame_count: usize,
    /// Size of the input file in bytes.
    pub bytes_in: u64,
    /// Size of the written output file in bytes.
    pub bytes_out: u64,
    /// Source `(width, height)`.
    pub src_dims: (u32, u32),
    /// Destination `(width, height)`.
    pub dst_dims: (u32, u32),
}

/// Read a Y4M clip, resize every frame to `(target_w, target_h)` honouring
/// `aspect`, and write the result to `output`.
///
/// `operation` labels the caller in error messages (matching the convention
/// every other harness entry point uses).
///
/// # Errors
///
/// Returns an error if the input is not a usable Y4M clip, if the target
/// dimensions are zero or the resize/pad/crop math fails, or if muxing/
/// writing fails. No output file is written unless the whole pass succeeds.
pub fn resize_clip_file(
    operation: &str,
    input: &Path,
    output: &Path,
    target_w: u32,
    target_h: u32,
    kernel: FilterKernel,
    aspect: AspectRatioMode,
) -> Result<ScaleStats> {
    let raw = std::fs::read(input)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;
    let bytes_in = raw.len() as u64;
    let clip = super::read_y4m_clip_from_bytes(operation, input, &raw)?;
    let src_dims = (clip.width(), clip.height());

    let resized = resize_clip(&clip, target_w, target_h, kernel, aspect)?;
    let frame_count = resized.frames.len();
    let dst_dims = (resized.width(), resized.height());

    let bytes_out = super::write_y4m_clip(output, &resized)?;

    Ok(ScaleStats {
        frame_count,
        bytes_in,
        bytes_out,
        src_dims,
        dst_dims,
    })
}

/// Resize every frame of `clip` to `(target_w, target_h)`, honouring `aspect`.
///
/// # Errors
///
/// Returns an error if the target dimensions are zero, the clip's chroma
/// format has no `ChromaLayout` counterpart at the computed sizes, or a
/// per-frame resample/pad/crop step fails.
pub fn resize_clip(
    clip: &PlanarClip,
    target_w: u32,
    target_h: u32,
    kernel: FilterKernel,
    aspect: AspectRatioMode,
) -> Result<PlanarClip> {
    if target_w == 0 || target_h == 0 {
        anyhow::bail!("target dimensions must be non-zero, got {target_w}x{target_h}");
    }
    let src_w = clip.layout.luma_w as u32;
    let src_h = clip.layout.luma_h as u32;
    if src_w == 0 || src_h == 0 {
        anyhow::bail!("source clip has zero dimensions");
    }

    let (content_w, content_h) = content_dimensions(src_w, src_h, target_w, target_h, aspect);

    let content_layout = ChromaLayout::for_chroma(clip.layout.chroma, content_w, content_h)
        .ok_or_else(|| chroma_unsupported(clip.layout.chroma))?;
    let target_layout = ChromaLayout::for_chroma(clip.layout.chroma, target_w, target_h)
        .ok_or_else(|| chroma_unsupported(clip.layout.chroma))?;

    let config = ResamplerConfig {
        filter: kernel,
        ..ResamplerConfig::default()
    };

    let mut out_frames = Vec::with_capacity(clip.frames.len());
    for (i, frame) in clip.frames.iter().enumerate() {
        let resampled = resample_frame(frame, content_layout, &config)
            .with_context(|| format!("failed to resample frame {i}"))?;
        let placed = place_content(&resampled, content_layout, target_layout, aspect)
            .with_context(|| format!("failed to compose frame {i} into {target_w}x{target_h}"))?;
        out_frames.push(placed);
    }

    let mut header = clip.header.clone();
    header.width = target_w;
    header.height = target_h;

    Ok(PlanarClip {
        layout: target_layout,
        header,
        frames: out_frames,
    })
}

fn chroma_unsupported(chroma: Y4mChroma) -> anyhow::Error {
    anyhow::anyhow!("Y4M chroma format '{chroma}' is not supported by the scaling pipeline")
}

/// Compute the "content" dimensions frames are resampled to before the final
/// pad/crop/identity step. See the module docs for why `Letterbox` and `Crop`
/// need different formulas here even though `oximedia_scaling`'s own
/// dimension calculator does not distinguish them.
fn content_dimensions(
    src_w: u32,
    src_h: u32,
    target_w: u32,
    target_h: u32,
    aspect: AspectRatioMode,
) -> (u32, u32) {
    match aspect {
        AspectRatioMode::Stretch => (target_w, target_h),
        AspectRatioMode::Crop => cover_dimensions(src_w, src_h, target_w, target_h),
        AspectRatioMode::Letterbox => contain_dimensions(src_w, src_h, target_w, target_h),
    }
}

/// The smallest size that covers `target_w x target_h` while preserving
/// `src_w x src_h`'s aspect ratio (result `>=` target on both axes).
fn cover_dimensions(src_w: u32, src_h: u32, target_w: u32, target_h: u32) -> (u32, u32) {
    let src_aspect = f64::from(src_w) / f64::from(src_h);
    let dst_aspect = f64::from(target_w) / f64::from(target_h);
    if src_aspect > dst_aspect {
        (
            ((f64::from(target_h) * src_aspect).round() as u32).max(target_w),
            target_h,
        )
    } else {
        (
            target_w,
            ((f64::from(target_w) / src_aspect).round() as u32).max(target_h),
        )
    }
}

/// The largest size that fits within `target_w x target_h` while preserving
/// `src_w x src_h`'s aspect ratio (result `<=` target on both axes).
fn contain_dimensions(src_w: u32, src_h: u32, target_w: u32, target_h: u32) -> (u32, u32) {
    let src_aspect = f64::from(src_w) / f64::from(src_h);
    let dst_aspect = f64::from(target_w) / f64::from(target_h);
    if src_aspect > dst_aspect {
        (
            target_w,
            ((f64::from(target_w) / src_aspect).round() as u32)
                .max(1)
                .min(target_h),
        )
    } else {
        (
            ((f64::from(target_h) * src_aspect).round() as u32)
                .max(1)
                .min(target_w),
            target_h,
        )
    }
}

/// Resample every plane of `frame` from its own layout to `dst_layout`'s
/// dimensions, independently per plane (so chroma is resampled at its own,
/// correctly-subsampled resolution rather than by naively halving the luma
/// scale factor).
fn resample_frame(
    frame: &PlanarFrame,
    dst_layout: ChromaLayout,
    config: &ResamplerConfig,
) -> Result<PlanarFrame> {
    let src_layout = frame.layout;
    let mut data = vec![0u8; dst_layout.frame_size()];

    for plane_idx in 0..src_layout.plane_count() {
        let (sw, sh) = src_layout
            .plane_dims(plane_idx)
            .ok_or_else(|| anyhow::anyhow!("source layout has no plane {plane_idx}"))?;
        let (dw, dh) = dst_layout
            .plane_dims(plane_idx)
            .ok_or_else(|| anyhow::anyhow!("destination layout has no plane {plane_idx}"))?;
        let src = frame
            .plane(plane_idx)
            .ok_or_else(|| anyhow::anyhow!("source frame is missing plane {plane_idx}"))?;

        let src_f32: Vec<f32> = src.iter().map(|&b| f32::from(b)).collect();
        // `Resampler::resize` normalises by the accumulated filter weight, so
        // it is scale-agnostic: raw 0..255 values in, raw values out, no
        // 0..1 normalisation round-trip needed.
        let dst_f32 =
            Resampler::resize(&src_f32, sw as u32, sh as u32, dw as u32, dh as u32, config);

        let doff = dst_layout.plane_offset(plane_idx).ok_or_else(|| {
            anyhow::anyhow!("destination layout has no offset for plane {plane_idx}")
        })?;
        let dlen = dst_layout.plane_len(plane_idx).ok_or_else(|| {
            anyhow::anyhow!("destination layout has no length for plane {plane_idx}")
        })?;
        if dst_f32.len() != dlen {
            anyhow::bail!(
                "resampled plane {plane_idx} produced {} samples, expected {dlen} for {dw}x{dh}",
                dst_f32.len()
            );
        }
        for (i, &v) in dst_f32.iter().enumerate() {
            data[doff + i] = v.round().clamp(0.0, 255.0) as u8;
        }
    }

    PlanarFrame::new(dst_layout, data)
}

/// Place resampled `content` (already at `content_layout`'s size) into a
/// frame at `target_layout`'s size, honouring `aspect`.
fn place_content(
    content: &PlanarFrame,
    content_layout: ChromaLayout,
    target_layout: ChromaLayout,
    aspect: AspectRatioMode,
) -> Result<PlanarFrame> {
    if content_layout == target_layout {
        // Stretch always lands here (content *is* the target size); letterbox
        // and crop land here too whenever the source already has the
        // target's aspect ratio, in which case there is nothing to pad/crop.
        return Ok(content.clone());
    }
    match aspect {
        AspectRatioMode::Stretch => anyhow::bail!(
            "internal error: stretch must resample directly to the target size \
             ({}x{} vs {}x{})",
            content_layout.luma_w,
            content_layout.luma_h,
            target_layout.luma_w,
            target_layout.luma_h
        ),
        AspectRatioMode::Letterbox => pad_to(content, content_layout, target_layout),
        AspectRatioMode::Crop => crop_to(content, content_layout, target_layout),
    }
}

/// Centre `content` inside a `target_layout`-sized frame, filling the
/// letterbox/pillarbox bars with full-range black.
fn pad_to(
    content: &PlanarFrame,
    content_layout: ChromaLayout,
    target_layout: ChromaLayout,
) -> Result<PlanarFrame> {
    let cfg = PadOperation::letterbox(
        content_layout.luma_w as u32,
        content_layout.luma_h as u32,
        target_layout.luma_w as u32,
        target_layout.luma_h as u32,
    )
    .ok_or_else(|| {
        anyhow::anyhow!(
            "content {}x{} does not fit within target {}x{} for letterboxing",
            content_layout.luma_w,
            content_layout.luma_h,
            target_layout.luma_w,
            target_layout.luma_h
        )
    })?;

    let mut data = vec![0u8; target_layout.frame_size()];
    fill_plane(&mut data, target_layout, 0, PAD_LUMA)?;
    if target_layout.chroma_len() > 0 {
        fill_plane(&mut data, target_layout, 1, PAD_CHROMA)?;
        fill_plane(&mut data, target_layout, 2, PAD_CHROMA)?;
    }
    if target_layout.has_alpha {
        fill_plane(
            &mut data,
            target_layout,
            target_layout.plane_count() - 1,
            255,
        )?;
    }

    let (h_sub, v_sub) = chroma_subsample(target_layout.chroma);
    let ox = cfg.left as usize;
    let oy = cfg.top as usize;

    blit_plane_rect(
        &content.data,
        content_layout,
        0,
        0,
        &mut data,
        target_layout,
        ox,
        oy,
        0,
        content_layout.luma_w,
        content_layout.luma_h,
    )?;
    if target_layout.chroma_len() > 0 {
        for plane_idx in [1, 2] {
            blit_plane_rect(
                &content.data,
                content_layout,
                0,
                0,
                &mut data,
                target_layout,
                ox / h_sub,
                oy / v_sub,
                plane_idx,
                content_layout.chroma_w,
                content_layout.chroma_h,
            )?;
        }
    }
    if target_layout.has_alpha {
        let idx = target_layout.plane_count() - 1;
        blit_plane_rect(
            &content.data,
            content_layout,
            0,
            0,
            &mut data,
            target_layout,
            ox,
            oy,
            idx,
            content_layout.luma_w,
            content_layout.luma_h,
        )?;
    }

    PlanarFrame::new(target_layout, data)
}

/// Crop `content` (which covers `target_layout`'s size on both axes) down to
/// a centred `target_layout`-sized frame.
fn crop_to(
    content: &PlanarFrame,
    content_layout: ChromaLayout,
    target_layout: ChromaLayout,
) -> Result<PlanarFrame> {
    let content_w = content_layout.luma_w;
    let content_h = content_layout.luma_h;
    let target_w = target_layout.luma_w;
    let target_h = target_layout.luma_h;

    let ox = content_w.saturating_sub(target_w) / 2;
    let oy = content_h.saturating_sub(target_h) / 2;
    let rect = CropRect::new(ox as u32, oy as u32, target_w as u32, target_h as u32);
    if !rect.fits_in(content_w as u32, content_h as u32) {
        anyhow::bail!(
            "content {content_w}x{content_h} is smaller than target {target_w}x{target_h}; \
             cannot crop"
        );
    }

    let (h_sub, v_sub) = chroma_subsample(target_layout.chroma);

    let mut data = vec![0u8; target_layout.frame_size()];
    blit_plane_rect(
        &content.data,
        content_layout,
        ox,
        oy,
        &mut data,
        target_layout,
        0,
        0,
        0,
        target_w,
        target_h,
    )?;
    if target_layout.chroma_len() > 0 {
        for plane_idx in [1, 2] {
            blit_plane_rect(
                &content.data,
                content_layout,
                ox / h_sub,
                oy / v_sub,
                &mut data,
                target_layout,
                0,
                0,
                plane_idx,
                target_layout.chroma_w,
                target_layout.chroma_h,
            )?;
        }
    }
    if target_layout.has_alpha {
        let idx = target_layout.plane_count() - 1;
        blit_plane_rect(
            &content.data,
            content_layout,
            ox,
            oy,
            &mut data,
            target_layout,
            0,
            0,
            idx,
            target_w,
            target_h,
        )?;
    }

    PlanarFrame::new(target_layout, data)
}

/// Integer luma:chroma downsampling ratio `(horizontal, vertical)` for a Y4M
/// chroma format. Mirrors the match in
/// [`ChromaLayout::for_chroma`](super::ChromaLayout::for_chroma) exactly.
const fn chroma_subsample(chroma: Y4mChroma) -> (usize, usize) {
    match chroma {
        Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => (2, 2),
        Y4mChroma::C422 => (2, 1),
        Y4mChroma::C444 | Y4mChroma::C444alpha | Y4mChroma::Mono => (1, 1),
    }
}

/// Fill plane `plane_idx` of a packed planar buffer with a constant value.
fn fill_plane(data: &mut [u8], layout: ChromaLayout, plane_idx: usize, value: u8) -> Result<()> {
    let off = layout
        .plane_offset(plane_idx)
        .ok_or_else(|| anyhow::anyhow!("no offset for plane {plane_idx}"))?;
    let len = layout
        .plane_len(plane_idx)
        .ok_or_else(|| anyhow::anyhow!("no length for plane {plane_idx}"))?;
    data[off..off + len].fill(value);
    Ok(())
}

/// Copy a `copy_w x copy_h` rectangle of plane `plane_idx` from `src` at
/// `(src_x, src_y)` into `dst` at `(dst_x, dst_y)`.
#[allow(clippy::too_many_arguments)]
fn blit_plane_rect(
    src: &[u8],
    src_layout: ChromaLayout,
    src_x: usize,
    src_y: usize,
    dst: &mut [u8],
    dst_layout: ChromaLayout,
    dst_x: usize,
    dst_y: usize,
    plane_idx: usize,
    copy_w: usize,
    copy_h: usize,
) -> Result<()> {
    let (sw, _) = src_layout
        .plane_dims(plane_idx)
        .ok_or_else(|| anyhow::anyhow!("source layout has no plane {plane_idx}"))?;
    let (dw, _) = dst_layout
        .plane_dims(plane_idx)
        .ok_or_else(|| anyhow::anyhow!("destination layout has no plane {plane_idx}"))?;
    let soff = src_layout
        .plane_offset(plane_idx)
        .ok_or_else(|| anyhow::anyhow!("source layout has no offset for plane {plane_idx}"))?;
    let doff = dst_layout
        .plane_offset(plane_idx)
        .ok_or_else(|| anyhow::anyhow!("destination layout has no offset for plane {plane_idx}"))?;

    for row in 0..copy_h {
        let s_start = soff + (src_y + row) * sw + src_x;
        let d_start = doff + (dst_y + row) * dw + dst_x;
        dst[d_start..d_start + copy_w].copy_from_slice(&src[s_start..s_start + copy_w]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A gradient 4:2:0 clip so resampling actually mixes distinct values.
    fn gradient_clip(width: u32, height: u32, frames: usize) -> PlanarClip {
        let h = header(width, height);
        let w = width as usize;
        let hh = height as usize;
        let cw = w.div_ceil(2);
        let ch = hh.div_ceil(2);
        let mut raw = Vec::new();
        for t in 0..frames {
            let mut buf = Vec::with_capacity(w * hh + 2 * cw * ch);
            for y in 0..hh {
                for x in 0..w {
                    buf.push(((x * 3 + y * 5 + t * 7) % 256) as u8);
                }
            }
            for plane in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        buf.push(((x * 2 + y + plane * 40) % 256) as u8);
                    }
                }
            }
            raw.push(buf);
        }
        PlanarClip::from_raw_frames(h, raw).expect("clip")
    }

    #[test]
    fn cover_dimensions_fills_and_overflows() {
        // 4:3 (1.333) source into 16:9 (1.778) target: the source is
        // proportionally narrower than the target, so covering the box means
        // fitting *width* to the target and letting height overflow (to be
        // cropped) — matches `oximedia_scaling`'s own
        // `test_calculate_dimensions_letterbox`, which asserts this exact
        // (1920, 1440) result for the same inputs.
        let (w, h) = cover_dimensions(1024, 768, 1920, 1080);
        assert_eq!(w, 1920);
        assert!(
            h > 1080,
            "cover height {h} must exceed the target for 4:3 into 16:9"
        );
    }

    #[test]
    fn contain_dimensions_fits_and_shrinks() {
        // Same inputs, opposite goal: fit entirely *within* the target, so
        // height matches the target and width shrinks to be pillarboxed.
        let (w, h) = contain_dimensions(1024, 768, 1920, 1080);
        assert_eq!(h, 1080);
        assert!(
            w < 1920,
            "contain width {w} must be under the target for 4:3 into 16:9"
        );
    }

    #[test]
    fn stretch_resizes_directly_to_target() {
        let clip = gradient_clip(16, 16, 2);
        let out = resize_clip(
            &clip,
            32,
            8,
            FilterKernel::Bilinear,
            AspectRatioMode::Stretch,
        )
        .expect("stretch resize");
        assert_eq!((out.width(), out.height()), (32, 8));
        assert_eq!(out.frames.len(), 2);
        for frame in &out.frames {
            assert_eq!(frame.data.len(), out.layout.frame_size());
        }
    }

    #[test]
    fn letterbox_pads_with_black_bars() {
        // 16x16 (square) into 32x16 (2:1): content stays 16x16, padded left/right.
        let clip = gradient_clip(16, 16, 1);
        let out = resize_clip(
            &clip,
            32,
            16,
            FilterKernel::Bilinear,
            AspectRatioMode::Letterbox,
        )
        .expect("letterbox resize");
        assert_eq!((out.width(), out.height()), (32, 16));
        let frame = &out.frames[0];
        let luma_w = out.layout.luma_w;
        // Column 0 must be inside a pad bar (full-range black).
        assert_eq!(frame.luma()[0], PAD_LUMA, "left bar must be padded black");
        assert_eq!(
            frame.luma()[luma_w - 1],
            PAD_LUMA,
            "right bar must be padded black"
        );
    }

    #[test]
    fn crop_covers_without_bars() {
        // 16x16 (square) into 32x16 (2:1) with Crop: no pixel may be pad-black
        // everywhere (crop fills the whole frame, unlike letterbox).
        let clip = gradient_clip(16, 16, 1);
        let out = resize_clip(&clip, 32, 16, FilterKernel::Bilinear, AspectRatioMode::Crop)
            .expect("crop resize");
        assert_eq!((out.width(), out.height()), (32, 16));
        // A gradient source cropped and scaled should not be a uniform bar of
        // pure PAD_LUMA down either edge column (unlike the letterbox case).
        let frame = &out.frames[0];
        let luma_w = out.layout.luma_w;
        let left_col_all_black =
            (0..out.layout.luma_h).all(|row| frame.luma()[row * luma_w] == PAD_LUMA);
        assert!(!left_col_all_black, "crop must not introduce a black bar");
    }

    #[test]
    fn resize_rejects_zero_target_dimensions() {
        let clip = gradient_clip(8, 8, 1);
        let err = resize_clip(
            &clip,
            0,
            8,
            FilterKernel::Bilinear,
            AspectRatioMode::Stretch,
        )
        .expect_err("zero width must be rejected");
        assert!(format!("{err}").contains("non-zero"));
    }
}
