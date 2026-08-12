//! Real AV1-backed AVIF pixel decode.
//!
//! Feeds the AV1 OBU bitstream(s) extracted by
//! [`super::AvifDecoder::extract_av1_payload`] through the crate's
//! bit-exact keyframe/intra decoder ([`crate::av1::Av1Decoder`], verified
//! against dav1d/aomdec) and assembles the result into an [`AvifImage`].
//!
//! Compiled only when the `av1` cargo feature is enabled — see
//! `AvifDecoder::decode` in `mod.rs` for the `#[cfg(not(feature = "av1"))]`
//! fallback used when it is not.

use super::{AvifImage, AvifPayload, YuvFormat};
use crate::av1::Av1Decoder;
use crate::error::CodecError;
use crate::frame::{Plane, VideoFrame};
use crate::traits::{DecoderConfig, VideoDecoder};
use oximedia_core::PixelFormat;

/// Decode a still AVIF image from its extracted AV1 payload(s).
///
/// Decodes the colour item first; if an alpha auxiliary item is present it
/// is decoded too — the two are inseparable: an image whose alpha channel
/// cannot be decoded is not silently returned as if it were fully opaque.
///
/// # Errors
///
/// Propagates the AV1 decoder's own honest errors (for example
/// `CodecError::UnsupportedFeature` for 10/12-bit, monochrome, or
/// non-4:2:0 streams — real-world AVIF alpha auxiliary images are
/// routinely encoded as monochrome AV1, which hits this gate today), with
/// a short prefix identifying which image item (colour or alpha) failed.
pub(super) fn decode_still_image(payload: &AvifPayload) -> Result<AvifImage, CodecError> {
    let color_frame = decode_av1_temporal_unit(&payload.color_obu)
        .map_err(|e| prefix_error(e, "AVIF color image"))?;
    let mut image = video_frame_to_avif_image(&color_frame)?;

    if let Some(alpha_obu) = &payload.alpha_obu {
        let alpha_frame = decode_av1_temporal_unit(alpha_obu)
            .map_err(|e| prefix_error(e, "AVIF alpha auxiliary image"))?;
        image.alpha_plane = Some(extract_alpha_samples(
            &alpha_frame,
            image.width,
            image.height,
        )?);
    }

    Ok(image)
}

/// Decode one AV1 temporal unit (a full OBU bitstream: sequence header
/// followed by a coded frame) to a single [`VideoFrame`] via the crate's
/// real, bit-exact keyframe/intra decoder.
fn decode_av1_temporal_unit(obu_data: &[u8]) -> Result<VideoFrame, CodecError> {
    let mut decoder = Av1Decoder::new(DecoderConfig::default())?;
    decoder.send_packet(obu_data, 0)?;
    decoder.receive_frame()?.ok_or_else(|| {
        CodecError::InvalidBitstream(
            "AV1 payload produced no frame (sequence header without a coded frame)".into(),
        )
    })
}

/// Prefixes an error with which AVIF image item it came from, preserving
/// the original [`CodecError`] variant (so callers matching on
/// `CodecError::UnsupportedFeature(_)` still see that variant, not a
/// generic string wrapper).
fn prefix_error(err: CodecError, context: &str) -> CodecError {
    match err {
        CodecError::UnsupportedFeature(msg) => {
            CodecError::UnsupportedFeature(format!("{context}: {msg}"))
        }
        CodecError::InvalidBitstream(msg) => {
            CodecError::InvalidBitstream(format!("{context}: {msg}"))
        }
        other => other,
    }
}

/// Convert a decoded [`VideoFrame`] into an [`AvifImage`]'s colour planes.
///
/// [`Av1Decoder`] reconstructs 8-bit 4:2:0 (profile 0) only today — its
/// private `planes_to_video_frame` helper hard-codes `PixelFormat::Yuv420p`
/// — every other surface fails honestly inside the AV1 decoder itself
/// before a frame is ever produced. The format check below is therefore
/// defensive rather than reachable: it keeps this conversion honest
/// instead of mislabelling depth/subsampling if that ever changes.
fn video_frame_to_avif_image(frame: &VideoFrame) -> Result<AvifImage, CodecError> {
    if frame.format != PixelFormat::Yuv420p {
        return Err(CodecError::UnsupportedFeature(format!(
            "AVIF decode: unsupported AV1 output pixel format {:?} (only 8-bit 4:2:0 is implemented)",
            frame.format
        )));
    }
    if frame.planes.len() != 3 {
        return Err(CodecError::Internal(format!(
            "AV1 decode produced {} planes, expected 3 (Y/U/V)",
            frame.planes.len()
        )));
    }

    Ok(AvifImage {
        width: frame.width,
        height: frame.height,
        depth: 8,
        yuv_format: YuvFormat::Yuv420,
        y_plane: plane_bytes(&frame.planes[0]),
        u_plane: plane_bytes(&frame.planes[1]),
        v_plane: plane_bytes(&frame.planes[2]),
        alpha_plane: None,
    })
}

/// Extract the alpha auxiliary image's luma plane as the alpha channel.
///
/// AVIF alpha images carry their sample values in the luma (Y) plane;
/// real-world encoders emit them as monochrome AV1 streams (no chroma
/// planes at all). The decoded alpha image must match the colour image's
/// dimensions exactly -- AVIF does not define a resampling rule for a
/// mismatched auxiliary alpha image, so a mismatch is bitstream corruption,
/// not something to silently crop/stretch around.
fn extract_alpha_samples(
    alpha_frame: &VideoFrame,
    color_width: u32,
    color_height: u32,
) -> Result<Vec<u8>, CodecError> {
    if alpha_frame.width != color_width || alpha_frame.height != color_height {
        return Err(CodecError::InvalidBitstream(format!(
            "AVIF alpha image size {}x{} does not match colour image size {}x{}",
            alpha_frame.width, alpha_frame.height, color_width, color_height
        )));
    }
    let luma = alpha_frame
        .planes
        .first()
        .ok_or_else(|| CodecError::Internal("AV1 alpha decode produced no planes".into()))?;
    Ok(plane_bytes(luma))
}

/// Copy plane samples into a tightly packed `Vec<u8>`, honoring `stride`.
///
/// [`Av1Decoder`]'s output planes are already tightly packed (`stride ==
/// width`; see `Av1Decoder::planes_to_video_frame`), so the common case is
/// a plain clone, but this stays correct (row-by-row, bounds-checked) if
/// that ever changes.
fn plane_bytes(plane: &Plane) -> Vec<u8> {
    let w = plane.width as usize;
    let h = plane.height as usize;
    if plane.stride == w {
        return plane.data.get(..w * h).unwrap_or(&plane.data[..]).to_vec();
    }
    let mut out = Vec::with_capacity(w * h);
    for row in 0..h {
        let start = row * plane.stride;
        let end = (start + w).min(plane.data.len());
        if start < end {
            out.extend_from_slice(&plane.data[start..end]);
        }
    }
    out
}
