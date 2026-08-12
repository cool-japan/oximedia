//! Lossy WebP (VP8 key-frame) decoding.
//!
//! A lossy WebP file is a RIFF container holding a single VP8 key frame
//! (RFC 6386): the `VP8 ` chunk payload *is* a spec-valid VP8 key-frame
//! bitstream, byte for byte. This module wires that payload to the existing
//! bit-exact VP8 key-frame decoder (`crate::vp8::decode_keyframe`, the same
//! pipeline `crate::vp8::Vp8Decoder` uses for real video streams) rather
//! than reimplementing any VP8 decoding here.
//!
//! # API shape
//!
//! Two entry points, mirroring the raw-bitstream / whole-container split
//! already established by the sibling lossless decoder
//! ([`crate::webp::vp8l_decoder::Vp8lDecoder::decode`] takes a raw `VP8L`
//! bitstream; RIFF unwrapping is the caller's job):
//!
//! - [`decode_vp8_keyframe`] — raw `VP8 ` chunk payload in, a YUV 4:2:0
//!   [`VideoFrame`] out. Thin wrapper around `vp8::decode_keyframe` that
//!   hands the reconstructed planes to a [`VideoFrame`] the same way
//!   `vp8::decoder::Vp8Decoder::decode_frame` does (same
//!   `Plane::with_dimensions` calls, same [`PixelFormat::Yuv420p`]).
//! - [`decode_vp8`] — a full WebP RIFF container in (`b"RIFF" ... b"WEBP"
//!   ...`), a [`VideoFrame`] out. Parses the container with
//!   [`crate::webp::riff::WebPContainer::parse`] — reused as-is rather than
//!   re-implementing FourCC/chunk-size/padding parsing, since that parser
//!   already exists, is unit-tested, and already guards against truncated
//!   chunks and bad RIFF/WEBP magic — extracts the `VP8 ` chunk, decodes it,
//!   and merges an `ALPH` chunk into the output if the container carries one
//!   (extended format: `VP8X` + `ALPH` + `VP8 `).
//!
//! # Output convention: YUV by default, RGBA only with alpha
//!
//! [`Vp8lDecoder::decode`](crate::webp::vp8l_decoder::Vp8lDecoder::decode)
//! always returns packed ARGB pixels, because VP8L's native colour space
//! *is* ARGB — there is no lossy conversion to avoid. VP8's native colour
//! space is YUV 4:2:0, so [`decode_vp8_keyframe`] and the no-alpha path of
//! [`decode_vp8`] return a YUV 4:2:0 [`VideoFrame`] rather than converting:
//! that keeps every pixel identical to the bit-exact reference reconstruction
//! (`dwebp -yuv`) the crate's own VP8 tests are pinned against
//! (`crates/oximedia-codec/tests/vp8_fixtures/`), instead of laundering it
//! through a YUV->RGB matrix that libwebp's own bit-exact contract has
//! nothing to say about. [`PixelFormat`] has no YUV-with-alpha variant, so
//! the one case that necessarily leaves YUV space is a `VP8X`+`ALPH`
//! extended file: there, [`decode_vp8`] converts to
//! [`PixelFormat::Rgba32`] and appends the decoded alpha plane, mirroring
//! `ImageDecoder::decode_webp`'s existing "RGB24 normally, RGBA32 only with
//! alpha" convention in `crate::image` (not reused directly here — that
//! module requires the separate `image-io` feature, and its plane-indexing
//! assumes even width/height; see `yuv420p_to_rgb24` below).
//!
//! # Alpha scope
//!
//! [`crate::webp::alpha::decode_alpha`] already draws its own honest line:
//! the uncompressed alpha mode (all four spatial filters: none/horizontal/
//! vertical/gradient) is fully implemented, while VP8L-compressed alpha
//! (`AlphaCompression::WebPLossless`) returns
//! `CodecError::UnsupportedFeature`. [`decode_vp8`] calls `decode_alpha`
//! and propagates whatever it returns — so VP8L-compressed alpha is
//! deferred here for exactly the same reason, not a new gap this module
//! introduces.
//!
//! # Feature gate
//!
//! This module requires the `vp8` feature, because `crate::vp8` itself is
//! declared `#[cfg(feature = "vp8")]` in `lib.rs` while `crate::webp` is
//! not: without the feature, `crate::vp8::decode_keyframe` does not exist
//! to call. The sibling lossy WebP *encoder*
//! ([`crate::webp::encoder::WebPLossyEncoder`]) has no such requirement —
//! it carries its own self-contained VP8 bitstream-writing tables and does
//! not depend on `crate::vp8` at all.

use crate::error::{CodecError, CodecResult};
use crate::frame::{FrameType, Plane, VideoFrame};
use crate::vp8::decode_keyframe;
use crate::webp::alpha::decode_alpha;
use crate::webp::riff::{ChunkType, WebPContainer};
use oximedia_core::convert::pixel::{ColorMatrix, PixelConverter};
use oximedia_core::PixelFormat;

/// Decode a raw VP8 key-frame bitstream (a WebP `VP8 ` chunk's payload,
/// without the RIFF wrapper) into a YUV 4:2:0 [`VideoFrame`].
///
/// This is the same reconstruction `crate::vp8::Vp8Decoder` produces for a
/// video key frame: `crate::vp8::decode_keyframe` does the actual RFC
/// 6386 decode, and the planes are handed to the frame exactly as
/// `vp8::decoder::Vp8Decoder::decode_frame` does.
///
/// # Errors
///
/// Returns [`CodecError::InvalidBitstream`] for a malformed header,
/// truncated partition, or a payload that is not a VP8 key frame (an inter
/// frame here would be a corrupt lossy WebP — a lossy WebP image is always
/// exactly one key frame).
pub fn decode_vp8_keyframe(payload: &[u8]) -> CodecResult<VideoFrame> {
    let image = decode_keyframe(payload)?;
    let chroma_width = image.chroma_width();
    let chroma_height = image.chroma_height();

    let mut frame = VideoFrame::new(PixelFormat::Yuv420p, image.width, image.height);
    frame.frame_type = FrameType::Key;
    frame.planes = vec![
        Plane::with_dimensions(image.y, image.width as usize, image.width, image.height),
        Plane::with_dimensions(image.u, chroma_width as usize, chroma_width, chroma_height),
        Plane::with_dimensions(image.v, chroma_width as usize, chroma_width, chroma_height),
    ];
    Ok(frame)
}

/// Decode a lossy WebP file: a RIFF container whose bitstream chunk is
/// `VP8 ` (simple lossy format), or a `VP8X` extended-format container
/// carrying a `VP8 ` bitstream chunk and, optionally, an `ALPH` alpha
/// chunk.
///
/// Returns a YUV 4:2:0 [`VideoFrame`] when no alpha channel is present, or
/// an RGBA32 [`VideoFrame`] when an `ALPH` chunk is merged in (see the
/// module docs for why the pixel format varies).
///
/// # Errors
///
/// - [`CodecError::InvalidBitstream`] for a corrupt RIFF/WEBP container, a
///   truncated chunk, or a container whose bitstream chunk is `VP8L`
///   (lossless) rather than `VP8 ` — decode that through
///   [`crate::webp::vp8l_decoder::Vp8lDecoder`] instead.
/// - Whatever [`decode_vp8_keyframe`] or
///   [`crate::webp::alpha::decode_alpha`] return for a malformed VP8
///   payload or alpha chunk (including
///   [`CodecError::UnsupportedFeature`] for VP8L-compressed alpha — see
///   the module docs).
pub fn decode_vp8(container_data: &[u8]) -> CodecResult<VideoFrame> {
    let container = WebPContainer::parse(container_data)?;
    let chunk = container.bitstream_chunk().ok_or_else(|| {
        CodecError::InvalidBitstream("WebP container has no VP8/VP8L bitstream chunk".into())
    })?;
    if chunk.chunk_type != ChunkType::Vp8 {
        return Err(CodecError::InvalidBitstream(format!(
            "decode_vp8 expected a lossy 'VP8 ' bitstream chunk, found '{}' -- \
             lossless WebP (VP8L) decodes through webp::vp8l_decoder::Vp8lDecoder instead",
            chunk.chunk_type
        )));
    }

    let frame = decode_vp8_keyframe(&chunk.data)?;

    match container.alpha_chunk() {
        Some(alph) => merge_alpha(frame, &alph.data),
        None => Ok(frame),
    }
}

/// Merge a decoded `ALPH` chunk into a YUV 4:2:0 frame, producing RGBA32.
fn merge_alpha(frame: VideoFrame, alph_chunk_data: &[u8]) -> CodecResult<VideoFrame> {
    let width = frame.width;
    let height = frame.height;
    let alpha = decode_alpha(alph_chunk_data, width, height)?;

    let rgb = yuv420p_to_rgb24(
        &frame.planes[0].data,
        &frame.planes[1].data,
        &frame.planes[2].data,
        width,
        height,
    );

    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for (i, px) in rgb.chunks_exact(3).enumerate() {
        rgba.extend_from_slice(px);
        rgba.push(alpha.get(i).copied().unwrap_or(255));
    }

    let stride = (width as usize) * 4;
    let mut out = VideoFrame::new(PixelFormat::Rgba32, width, height);
    out.frame_type = FrameType::Key;
    out.planes = vec![Plane::with_dimensions(rgba, stride, width, height)];
    Ok(out)
}

/// Convert YUV 4:2:0 planes to packed RGB24, addressing chroma with the
/// same `div_ceil(2)` convention VP8 itself uses for odd frame dimensions
/// (`DecodedImage::chroma_width`/`chroma_height`, mirrored by
/// [`decode_vp8_keyframe`]'s plane hand-off above).
///
/// This crate has two other YUV->RGB helpers
/// (`oximedia_core::convert::pixel::yuv420p_to_rgb24` and
/// `crate::image::convert_yuv420p_to_rgb`); both were fixed alongside the
/// odd-width `ImageDecoder` lossy-WebP path to share this same
/// ceiling-division (`div_ceil(2)`) chroma convention instead of floor
/// division, so all three now agree on which chroma sample an odd
/// width/height reads. `crate::image::convert_yuv420p_to_rgb` additionally
/// now rejects a too-short chroma plane with a typed [`CodecError`] instead
/// of indexing it; `oximedia_core`'s `yuv420p_to_rgb24` keeps its
/// documented `assert_eq!`/panic precondition contract unchanged (an
/// established public API whose panic-on-misuse behaviour is intentional,
/// not touched by that fix).
///
/// This local loop still exists rather than calling either of them,
/// primarily because it uses [`PixelConverter`] with
/// [`ColorMatrix::Bt709`] — the integer-LUT BT.709 implementation
/// [`decode_vp8`]'s `ALPH`-chunk merge path (`merge_alpha`, above) needs to
/// stay pinned bit-exact against (this module's own
/// `yuv420p_to_rgb24_matches_pixel_converter_and_handles_odd_dimensions`
/// test below, and `webp_vp8_lossy.rs`'s alpha-merge integration test, both
/// assert against [`PixelConverter`] directly). `crate::image`'s BT.709
/// conversion instead uses its own direct f32 formula, which rounds
/// individual pixels differently; calling it here would shift the merged
/// RGBA output away from the `dwebp`-pinned reference by rounding noise, not
/// just change how chroma is indexed. A secondary reason: this loop takes
/// raw `&[u8]` Y/U/V plane slices (as decoded straight off `merge_alpha`'s
/// `VideoFrame`) and tolerates a too-short chroma plane by falling back to
/// neutral 128 via `.get()` rather than erroring, which suits this
/// best-effort alpha-merge caller better than a hard error would.
fn yuv420p_to_rgb24(y: &[u8], u: &[u8], v: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let chroma_width = width.div_ceil(2) as usize;
    let converter = PixelConverter::new(ColorMatrix::Bt709);

    let mut rgb = vec![0u8; w * h * 3];
    for row in 0..h {
        for col in 0..w {
            let y_val = y[row * w + col];
            let chroma_idx = (row / 2) * chroma_width + (col / 2);
            let u_val = u.get(chroma_idx).copied().unwrap_or(128);
            let v_val = v.get(chroma_idx).copied().unwrap_or(128);
            let (r, g, b) = converter.yuv_to_rgb(y_val, u_val, v_val);

            let out = (row * w + col) * 3;
            rgb[out] = r;
            rgb[out + 1] = g;
            rgb[out + 2] = b;
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webp::riff::WebPWriter;

    // These unit tests cover the container-dispatch plumbing (chunk-type
    // checks, error typing) with small synthetic inputs that do not need a
    // real VP8 bitstream. Bit-exact decode of real key frames (wrapped in a
    // RIFF/WebP container built in-test) is covered by the integration test
    // `tests/webp_vp8_lossy.rs`, which links against the crate's public API
    // and reuses `tests/vp8_fixtures/mod.rs`.

    #[test]
    fn decode_vp8_rejects_vp8l_chunk_with_typed_error() {
        // A minimal (bogus) VP8L payload is enough: decode_vp8 must reject
        // it by chunk type before ever trying to parse it as VP8L.
        let vp8l_payload = vec![0x2Fu8, 0, 0, 0, 0];
        let webp = WebPWriter::write_lossless(&vp8l_payload);

        let err = match decode_vp8(&webp) {
            Err(e) => e,
            Ok(_) => panic!("a VP8L container must not decode through decode_vp8"),
        };
        assert!(
            matches!(err, CodecError::InvalidBitstream(_)),
            "expected InvalidBitstream, got {err:?}"
        );
        assert!(
            err.to_string().contains("VP8 "),
            "error should name the expected chunk type, got: {err}"
        );
    }

    #[test]
    fn decode_vp8_rejects_truncated_chunk_with_typed_error() {
        // A `VP8 ` chunk whose declared size overruns the buffer: the
        // reused `WebPContainer::parse` truncation guard must catch this
        // before any VP8 decode is attempted.
        let mut webp = WebPWriter::write_lossy(&[0x10, 0x00, 0x00, 0x9D, 0x01, 0x2A]);
        // RIFF header (12 bytes) + chunk FourCC (4 bytes): chunk-size field
        // starts right after.
        let size_offset = 12 + 4;
        webp[size_offset..size_offset + 4].copy_from_slice(&9999u32.to_le_bytes());

        let err = match decode_vp8(&webp) {
            Err(e) => e,
            Ok(_) => panic!("a truncated chunk must not decode"),
        };
        assert!(
            matches!(err, CodecError::InvalidBitstream(_)),
            "expected InvalidBitstream, got {err:?}"
        );
    }

    #[test]
    fn decode_vp8_no_bitstream_chunk_is_typed_error() {
        // A well-formed RIFF/WEBP header with zero chunks cannot happen via
        // WebPContainer::parse (it already rejects that), so this exercises
        // the same guard indirectly: garbage payload after RIFF/WEBP magic
        // that fails chunk parsing entirely still yields InvalidBitstream,
        // never a panic or fabricated frame.
        let mut data = Vec::new();
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(b"WEBP");
        assert!(matches!(
            decode_vp8(&data),
            Err(CodecError::InvalidBitstream(_))
        ));
    }

    #[test]
    fn yuv420p_to_rgb24_matches_pixel_converter_and_handles_odd_dimensions() {
        // 3x3 (odd) so chroma is 2x2 under div_ceil(2), not 1x1 under floor
        // division -- this is exactly the case the module doc explains.
        let width = 3u32;
        let height = 3u32;
        let y = vec![200u8; 9];
        let u = vec![90u8; 4];
        let v = vec![160u8; 4];

        let rgb = yuv420p_to_rgb24(&y, &u, &v, width, height);
        assert_eq!(rgb.len(), 9 * 3);

        let converter = PixelConverter::new(ColorMatrix::Bt709);
        let expected = converter.yuv_to_rgb(200, 90, 160);
        assert_eq!((rgb[0], rgb[1], rgb[2]), expected);
        // Bottom-right pixel (row=2, col=2) maps to chroma (1,1) = index 3.
        let last = 8 * 3;
        assert_eq!((rgb[last], rgb[last + 1], rgb[last + 2]), expected);
    }
}
