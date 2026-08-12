//! Bridges between the harness's packed planar frames and the frame types the
//! rest of the stack speaks.
//!
//! Two bridges live here:
//!
//! * **planar ⇄ [`oximedia_codec::VideoFrame`]** — the currency of
//!   `oximedia-graph` filters ([`oximedia_graph::frame::FilterFrame`] wraps a
//!   `VideoFrame`). `VideoFrame` is built only through its public constructor
//!   and `planes` field; nothing in `oximedia-codec` is modified.
//! * **planar ⇄ single-channel `f32` planes** — the currency of the numeric
//!   image ops (denoise, sharpen, …) that later slices wire in.
//!
//! # Why the conversion back is layout-driven
//!
//! Several graph filters rebuild planes with [`oximedia_codec::Plane::new`],
//! which sets `width`/`height` to `0` (only `data` and `stride` survive). The
//! reverse bridge therefore reconstructs geometry from the [`ChromaLayout`]
//! the frame came in with, never from `plane.width` / `plane.height`, and
//! validates every plane's byte length against it. A format-mapping or
//! rounding disagreement surfaces as an honest error instead of silently
//! corrupted pixels.

use anyhow::Result;
use oximedia_codec::VideoFrame;
use oximedia_container::demux::y4m::Y4mChroma;
use oximedia_core::{PixelFormat, Rational, Timestamp};

use super::{ChromaLayout, PlanarFrame};

/// A single-channel plane in normalised `f32` (`0.0..=1.0`).
#[derive(Debug, Clone, PartialEq)]
pub struct F32Plane {
    /// Plane width in samples.
    pub width: usize,
    /// Plane height in samples.
    pub height: usize,
    /// Row-major samples, `width * height` long.
    pub data: Vec<f32>,
}

/// Map a Y4M chroma layout onto the matching planar [`PixelFormat`].
///
/// # Errors
///
/// Returns an error for chroma formats with no planar 8-bit `PixelFormat`
/// counterpart (currently only `C444alpha`, which has no YUVA variant).
pub fn pixel_format_for(layout: &ChromaLayout) -> Result<PixelFormat> {
    match layout.chroma {
        Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => {
            Ok(PixelFormat::Yuv420p)
        }
        Y4mChroma::C422 => Ok(PixelFormat::Yuv422p),
        Y4mChroma::C444 => Ok(PixelFormat::Yuv444p),
        Y4mChroma::Mono => Ok(PixelFormat::Gray8),
        Y4mChroma::C444alpha => anyhow::bail!(
            "Y4M chroma format 'C444alpha' has no 8-bit planar PixelFormat counterpart in \
             oximedia-codec, so it cannot be handed to a video filter. Convert the clip to \
             C444 first (e.g. `oximedia transcode -i input.y4m -o output.y4m`)."
        ),
    }
}

/// Reject geometries the graph video filters cannot composite onto correctly.
///
/// The filters in `oximedia-graph` address chroma samples as
/// `width / h_sub` (floor division) while plane allocation — and this
/// harness — round chroma dimensions *up*. For odd dimensions the two
/// disagree and overlay writes land at the wrong offsets, so subsampled
/// formats are restricted to even dimensions on the subsampled axes.
///
/// # Errors
///
/// Returns an error naming the offending chroma format or dimension.
pub fn require_compositable(operation: &str, layout: &ChromaLayout) -> Result<()> {
    let format = pixel_format_for(layout)?;
    let (h_sub, v_sub) = format.chroma_subsampling();

    if h_sub > 1 && layout.luma_w % (h_sub as usize) != 0 {
        anyhow::bail!(
            "{operation} needs a frame width divisible by {h_sub} for {} chroma, but the clip \
             is {}x{}. Crop or pad it first \
             (e.g. `oximedia transcode -i input.y4m -vf crop=... output.y4m`).",
            layout.chroma,
            layout.luma_w,
            layout.luma_h
        );
    }
    if v_sub > 1 && layout.luma_h % (v_sub as usize) != 0 {
        anyhow::bail!(
            "{operation} needs a frame height divisible by {v_sub} for {} chroma, but the clip \
             is {}x{}. Crop or pad it first \
             (e.g. `oximedia transcode -i input.y4m -vf crop=... output.y4m`).",
            layout.chroma,
            layout.luma_w,
            layout.luma_h
        );
    }
    Ok(())
}

/// Build a [`VideoFrame`] from a packed planar frame.
///
/// `frame_index`, `fps_num` and `fps_den` set a real presentation timestamp
/// (`pts = frame_index` in a `fps_den / fps_num` timebase).
///
/// # Errors
///
/// Returns an error if the chroma format has no `PixelFormat` counterpart or
/// if the allocated planes disagree with the packed frame's geometry.
pub fn planar_to_video_frame(
    frame: &PlanarFrame,
    frame_index: usize,
    fps_num: u32,
    fps_den: u32,
) -> Result<VideoFrame> {
    let layout = frame.layout;
    let format = pixel_format_for(&layout)?;

    let mut video = VideoFrame::new(format, layout.luma_w as u32, layout.luma_h as u32);
    video.allocate();

    if video.planes.len() != layout.plane_count() {
        anyhow::bail!(
            "PixelFormat {format:?} allocated {} planes but the Y4M layout has {}",
            video.planes.len(),
            layout.plane_count()
        );
    }

    for index in 0..layout.plane_count() {
        let src = frame.plane(index).ok_or_else(|| {
            anyhow::anyhow!(
                "packed frame is missing plane {index} for {:?}",
                layout.chroma
            )
        })?;
        let dst = &mut video.planes[index];
        if dst.data.len() != src.len() {
            anyhow::bail!(
                "plane {index} of PixelFormat {format:?} allocates {} bytes but the Y4M layout \
                 packs {} bytes",
                dst.data.len(),
                src.len()
            );
        }
        dst.data.copy_from_slice(src);
    }

    let den = i64::from(fps_den.max(1));
    let num = i64::from(fps_num.max(1));
    video.timestamp = Timestamp::new(frame_index as i64, Rational::new(den, num));

    Ok(video)
}

/// Pack a [`VideoFrame`]'s planes back into a planar frame with `layout`.
///
/// Geometry comes from `layout`, not from the frame's `Plane` metadata: graph
/// filters routinely rebuild planes with [`oximedia_codec::Plane::new`], which zeroes
/// `width`/`height`.
///
/// # Errors
///
/// Returns an error if the frame's dimensions, plane count or any plane's byte
/// length disagrees with `layout`.
pub fn video_frame_to_planar(video: &VideoFrame, layout: ChromaLayout) -> Result<PlanarFrame> {
    if video.width as usize != layout.luma_w || video.height as usize != layout.luma_h {
        anyhow::bail!(
            "filter returned a {}x{} frame but the clip layout is {}x{}",
            video.width,
            video.height,
            layout.luma_w,
            layout.luma_h
        );
    }
    if video.planes.len() != layout.plane_count() {
        anyhow::bail!(
            "filter returned {} planes but the clip layout has {}",
            video.planes.len(),
            layout.plane_count()
        );
    }

    let mut data = vec![0u8; layout.frame_size()];
    for index in 0..layout.plane_count() {
        let offset = layout
            .plane_offset(index)
            .ok_or_else(|| anyhow::anyhow!("layout has no offset for plane {index}"))?;
        let len = layout
            .plane_len(index)
            .ok_or_else(|| anyhow::anyhow!("layout has no length for plane {index}"))?;
        let src = &video.planes[index].data;
        if src.len() != len {
            anyhow::bail!(
                "filter returned {} bytes for plane {index} but the clip layout expects {len}",
                src.len()
            );
        }
        data[offset..offset + len].copy_from_slice(src);
    }

    PlanarFrame::new(layout, data)
}

/// Extract plane `index` as a normalised `f32` plane (`0.0..=1.0`).
///
/// # Errors
///
/// Returns an error if the layout has no such plane.
pub fn plane_to_f32(frame: &PlanarFrame, index: usize) -> Result<F32Plane> {
    let (width, height) = frame
        .layout
        .plane_dims(index)
        .ok_or_else(|| anyhow::anyhow!("frame layout has no plane {index}"))?;
    let bytes = frame
        .plane(index)
        .ok_or_else(|| anyhow::anyhow!("frame layout has no plane {index}"))?;
    Ok(F32Plane {
        width,
        height,
        data: bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
    })
}

/// Write a normalised `f32` plane back into plane `index`, clamping to
/// `0.0..=1.0` and rounding to the nearest 8-bit value.
///
/// # Errors
///
/// Returns an error if the layout has no such plane or the dimensions differ.
pub fn plane_from_f32(frame: &mut PlanarFrame, index: usize, plane: &F32Plane) -> Result<()> {
    let (width, height) = frame
        .layout
        .plane_dims(index)
        .ok_or_else(|| anyhow::anyhow!("frame layout has no plane {index}"))?;
    if plane.width != width || plane.height != height {
        anyhow::bail!(
            "f32 plane is {}x{} but plane {index} of this frame is {width}x{height}",
            plane.width,
            plane.height
        );
    }
    if plane.data.len() != width * height {
        anyhow::bail!(
            "f32 plane declares {width}x{height} but carries {} samples",
            plane.data.len()
        );
    }
    let dst = frame
        .plane_mut(index)
        .ok_or_else(|| anyhow::anyhow!("frame layout has no plane {index}"))?;
    for (out, &value) in dst.iter_mut().zip(plane.data.iter()) {
        *out = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_harness::ChromaLayout;
    use oximedia_codec::Plane;

    fn layout_420(w: usize, h: usize) -> ChromaLayout {
        ChromaLayout::for_chroma(Y4mChroma::C420jpeg, w as u32, h as u32).expect("420 layout")
    }

    fn sample_frame(layout: ChromaLayout) -> PlanarFrame {
        let data = (0..layout.frame_size()).map(|i| (i % 251) as u8).collect();
        PlanarFrame::new(layout, data).expect("frame")
    }

    #[test]
    fn video_frame_round_trip_is_bit_exact() {
        let layout = layout_420(16, 8);
        let frame = sample_frame(layout);

        let video = planar_to_video_frame(&frame, 3, 25, 1).expect("to VideoFrame");
        assert_eq!(video.format, PixelFormat::Yuv420p);
        assert_eq!(video.width, 16);
        assert_eq!(video.height, 8);
        assert_eq!(video.planes.len(), 3);
        assert_eq!(video.timestamp.pts, 3);

        let back = video_frame_to_planar(&video, layout).expect("back to planar");
        assert_eq!(back.data, frame.data);
    }

    /// Planes rebuilt with `Plane::new` (zeroed width/height) must still
    /// convert back — geometry comes from the layout.
    #[test]
    fn round_trip_survives_zeroed_plane_metadata() {
        let layout = layout_420(16, 8);
        let frame = sample_frame(layout);
        let mut video = planar_to_video_frame(&frame, 0, 30, 1).expect("to VideoFrame");
        for plane in &mut video.planes {
            *plane = Plane::new(plane.data.clone(), plane.stride);
        }
        let back = video_frame_to_planar(&video, layout).expect("back to planar");
        assert_eq!(back.data, frame.data);
    }

    #[test]
    fn mismatched_plane_length_is_an_error() {
        let layout = layout_420(16, 8);
        let frame = sample_frame(layout);
        let mut video = planar_to_video_frame(&frame, 0, 25, 1).expect("to VideoFrame");
        video.planes[1].data.truncate(3);
        let err = video_frame_to_planar(&video, layout).expect_err("must reject");
        assert!(format!("{err}").contains("plane 1"), "got: {err}");
    }

    #[test]
    fn f32_plane_round_trip() {
        let layout = layout_420(8, 8);
        let mut frame = sample_frame(layout);
        let original = frame.data.clone();

        let plane = plane_to_f32(&frame, 0).expect("to f32");
        assert_eq!(plane.width, 8);
        assert_eq!(plane.height, 8);
        plane_from_f32(&mut frame, 0, &plane).expect("from f32");
        assert_eq!(
            frame.data, original,
            "u8 → f32 → u8 must round-trip exactly"
        );
    }

    #[test]
    fn compositable_guard_rejects_odd_420_dimensions() {
        let odd = layout_420(15, 8);
        let err = require_compositable("timecode burn", &odd).expect_err("odd width must reject");
        assert!(format!("{err}").contains("divisible by 2"), "got: {err}");

        let even = layout_420(16, 8);
        require_compositable("timecode burn", &even).expect("even geometry is fine");
    }

    #[test]
    fn alpha_chroma_has_no_pixel_format() {
        let layout = ChromaLayout::for_chroma(Y4mChroma::C444alpha, 8, 8).expect("layout");
        let err = pixel_format_for(&layout).expect_err("C444alpha must be refused");
        assert!(format!("{err}").contains("C444alpha"), "got: {err}");
    }
}
