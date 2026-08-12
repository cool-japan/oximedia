//! Lossy WebP (VP8 key-frame) container decode tests.
//!
//! `webp::vp8_decoder::decode_vp8_keyframe` decodes a raw VP8 key-frame
//! payload directly (already covered bit-exact by `vp8_real_bitstream.rs`
//! through `Vp8Decoder`, which calls the same underlying
//! `vp8::decode_keyframe`). What is unique to this file is the *container*
//! layer: wrapping the same fixture payloads in a hand-built RIFF/WebP
//! header, unwrapping them through `webp::decode_vp8`, and checking that
//! (a) the reconstructed planes are still bit-exact against the `dwebp -yuv`
//! reference (`GRAD32_EXPECTED_*` / `VPX_KF_EXPECTED_*`), (b) they are
//! pixel-identical to decoding the same payload directly (proving the RIFF
//! unwrap does not shift or corrupt the bitstream), (c) malformed containers
//! produce typed errors rather than panics or fabricated frames, and (d) an
//! uncompressed `ALPH` chunk merges into an RGBA32 output.
//!
//! This file also owns the *encoder* round trip: `WebPLossyEncoder`'s own
//! output fed back through `decode_vp8_keyframe`. Nothing in the tree
//! decoded that output until these tests existed, which is how the encoder
//! came to emit frames no conforming decoder could read at all — see
//! `test_encode_rgb_decodes_back_with_plausible_luma`.

#![cfg(feature = "vp8")]

// `vp8_fixtures` is shared with `vp8_real_bitstream.rs`; this file's tests
// exercise the RIFF *container* layer specifically (see the module docs
// above) and so intentionally do not use every fixture in the module —
// `GRAD32_SOURCE_Y` (PSNR-vs-source bound) and `VPX_INTER_FRAME_VP8`
// (honest inter-frame error) are already covered at the `vp8::dec` /
// `Vp8Decoder` level by `vp8_real_bitstream.rs`, and re-asserting them here
// would test the container-parsing-independent VP8 pipeline a second time
// rather than anything specific to this file.
#[allow(dead_code)]
mod vp8_fixtures;

use oximedia_codec::error::CodecError;
use oximedia_codec::frame::VideoFrame;
use oximedia_codec::webp::{decode_vp8, decode_vp8_keyframe, encode_alpha, WebPWriter};
use oximedia_core::convert::pixel::{ColorMatrix, PixelConverter};
use oximedia_core::PixelFormat;
use vp8_fixtures as fx;

/// Asserts a decoded plane equals the reference bit-exactly, with a
/// diagnostic that pinpoints the first mismatch.
fn assert_plane_bit_exact(name: &str, got: &[u8], want: &[u8]) {
    assert_eq!(
        got.len(),
        want.len(),
        "{name}: plane size mismatch (got {}, want {})",
        got.len(),
        want.len()
    );
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!(
            "{name}: first mismatch at index {i}: got {}, want {}",
            got[i], want[i]
        );
    }
}

/// Wraps a raw VP8 key-frame payload in a hand-built simple-lossy RIFF/WebP
/// container, decodes it through `decode_vp8`, and checks the result
/// against both the libwebp reference planes and a direct
/// `decode_vp8_keyframe` decode of the same unwrapped payload.
fn verify_container_roundtrip(
    tag: &str,
    payload: &[u8],
    width: u32,
    height: u32,
    expected_y: &[u8],
    expected_u: &[u8],
    expected_v: &[u8],
) -> VideoFrame {
    let webp = WebPWriter::write_lossy(payload);

    let via_container = decode_vp8(&webp).unwrap_or_else(|e| {
        panic!("{tag}: decode_vp8 on a RIFF-wrapped payload must succeed, got {e}")
    });
    let via_raw_payload = decode_vp8_keyframe(payload)
        .unwrap_or_else(|e| panic!("{tag}: decode_vp8_keyframe must succeed, got {e}"));

    assert_eq!(via_container.width, width, "{tag}: width");
    assert_eq!(via_container.height, height, "{tag}: height");
    assert_eq!(
        via_container.format,
        PixelFormat::Yuv420p,
        "{tag}: no-alpha decode must stay in native YUV, not be converted to RGB"
    );
    assert!(
        via_container.is_keyframe(),
        "{tag}: must be marked as a key frame"
    );
    assert_eq!(
        via_container.planes.len(),
        3,
        "{tag}: YUV 4:2:0 has three planes"
    );

    // 1. Bit-exact against the libwebp (dwebp -yuv) reference reconstruction.
    assert_plane_bit_exact(
        &format!("{tag}.Y"),
        via_container.plane(0).data(),
        expected_y,
    );
    assert_plane_bit_exact(
        &format!("{tag}.U"),
        via_container.plane(1).data(),
        expected_u,
    );
    assert_plane_bit_exact(
        &format!("{tag}.V"),
        via_container.plane(2).data(),
        expected_v,
    );

    // 2. The RIFF container layer must not shift or corrupt the bitstream:
    //    decoding through the container and decoding the raw payload
    //    directly must produce pixel-identical planes.
    for i in 0..3 {
        assert_plane_bit_exact(
            &format!("{tag}: container vs raw-payload plane {i}"),
            via_container.plane(i).data(),
            via_raw_payload.plane(i).data(),
        );
    }

    via_container
}

#[test]
fn test_decode_vp8_grad32_bit_exact_through_riff_container() {
    verify_container_roundtrip(
        "grad32",
        &fx::GRAD32_VP8,
        32,
        32,
        &fx::GRAD32_EXPECTED_Y,
        &fx::GRAD32_EXPECTED_U,
        &fx::GRAD32_EXPECTED_V,
    );
}

#[test]
fn test_decode_vp8_tex48x40_bit_exact_through_riff_container() {
    // 48x40: height is not a macroblock multiple, so this also proves the
    // RIFF container layer survives the bottom-macroblock-row crop.
    verify_container_roundtrip(
        "tex48x40",
        &fx::TEX48X40_VP8,
        48,
        40,
        &fx::TEX48X40_EXPECTED_Y,
        &fx::TEX48X40_EXPECTED_U,
        &fx::TEX48X40_EXPECTED_V,
    );
}

#[test]
fn test_decode_vp8_vpx_keyframe_bit_exact_through_riff_container() {
    verify_container_roundtrip(
        "vpx_kf",
        &fx::VPX_KEYFRAME_VP8,
        48,
        48,
        &fx::VPX_KF_EXPECTED_Y,
        &fx::VPX_KF_EXPECTED_U,
        &fx::VPX_KF_EXPECTED_V,
    );
}

#[test]
fn test_decode_vp8_truncated_chunk_is_typed_error_not_panic() {
    let mut webp = WebPWriter::write_lossy(&fx::GRAD32_VP8);

    // Corrupt the declared chunk size (RIFF header 12 bytes + FourCC 4
    // bytes) so it overruns the actual buffer -- the reused
    // `WebPContainer::parse` truncation guard must catch this.
    let size_offset = 12 + 4;
    webp[size_offset..size_offset + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    let err = match decode_vp8(&webp) {
        Err(e) => e,
        Ok(_) => panic!("a chunk declaring an impossible size must not decode"),
    };
    assert!(
        matches!(err, CodecError::InvalidBitstream(_)),
        "expected a typed InvalidBitstream error, got {err:?}"
    );
}

#[test]
fn test_decode_vp8_bad_riff_magic_is_typed_error_not_panic() {
    let mut webp = WebPWriter::write_lossy(&fx::GRAD32_VP8);
    webp[0] = b'X'; // corrupt "RIFF" magic
    assert!(
        matches!(decode_vp8(&webp), Err(CodecError::InvalidBitstream(_))),
        "a corrupt RIFF container must produce a typed error, never a fabricated frame"
    );
}

#[test]
fn test_decode_vp8_extended_uncompressed_alpha_merges_into_rgba32() {
    let width = 32u32;
    let height = 32u32;

    // A small deterministic alpha plane (not uniform, so a constant-fill
    // regression would be caught): a diagonal ramp.
    let alpha_plane: Vec<u8> = (0..width * height)
        .map(|i| {
            let x = i % width;
            let y = i / width;
            ((x + y) % 256) as u8
        })
        .collect();
    let alph_chunk = encode_alpha(&alpha_plane, width, height).expect("encode_alpha must succeed");

    let webp = WebPWriter::write_extended(&fx::GRAD32_VP8, Some(&alph_chunk), width, height);
    let frame = decode_vp8(&webp).expect("extended VP8X+ALPH+VP8 container must decode");

    assert_eq!(
        frame.format,
        PixelFormat::Rgba32,
        "an ALPH chunk must promote the output to RGBA32"
    );
    assert_eq!(frame.width, width);
    assert_eq!(frame.height, height);
    assert_eq!(frame.planes.len(), 1, "RGBA32 is a single packed plane");
    let rgba = frame.plane(0).data();
    assert_eq!(rgba.len(), (width * height * 4) as usize);

    // Alpha channel must survive the merge bit-exact (uncompressed ALPH,
    // "none" filter is the cheapest and thus what `encode_alpha` should
    // pick for this ramp, but check the *decoded* values regardless of
    // which filter got chosen).
    for i in 0..(width * height) as usize {
        assert_eq!(
            rgba[i * 4 + 3],
            alpha_plane[i],
            "alpha byte at pixel {i} must round-trip exactly"
        );
    }

    // RGB channels: cross-check the top-left pixel against the libwebp
    // reference Y/U/V for grad32, run through the same BT.709 matrix, as an
    // independent check that plane threading (not just alpha) is correct.
    let converter = PixelConverter::new(ColorMatrix::Bt709);
    let (r, g, b) = converter.yuv_to_rgb(
        fx::GRAD32_EXPECTED_Y[0],
        fx::GRAD32_EXPECTED_U[0],
        fx::GRAD32_EXPECTED_V[0],
    );
    assert_eq!((rgba[0], rgba[1], rgba[2]), (r, g, b), "top-left RGB pixel");
}

/// Builds a gray gradient: R == G == B, so the BT.601 luma
/// `0.299R + 0.587G + 0.114B` this encoder applies reduces exactly to the
/// gray value. That keeps the luma bound below a measure of *codec* error
/// rather than of colour-matrix rounding.
fn gray_gradient(width: u32, height: u32) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = ((x * 255 / width) + (y * 60 / height)).min(255) as u8;
            rgb.extend_from_slice(&[value, value, value]);
        }
    }
    rgb
}

/// Encodes `rgb` with `WebPLossyEncoder`, decodes the result back through
/// `decode_vp8_keyframe`, and returns the mean absolute luma error against
/// the source gray values.
fn encode_decode_luma_mae(width: u32, height: u32, rgb: &[u8]) -> f64 {
    use oximedia_codec::webp::WebPLossyEncoder;

    let vp8_data = WebPLossyEncoder::new(90)
        .encode_rgb(rgb, width, height)
        .unwrap_or_else(|e| panic!("{width}x{height}: encode_rgb must succeed, got {e}"));

    let frame = decode_vp8_keyframe(&vp8_data).unwrap_or_else(|e| {
        panic!("{width}x{height}: this encoder's own output must decode, got {e}")
    });

    assert_eq!(frame.width, width, "{width}x{height}: decoded width");
    assert_eq!(frame.height, height, "{width}x{height}: decoded height");
    assert_eq!(
        frame.format,
        PixelFormat::Yuv420p,
        "{width}x{height}: decoded pixel format"
    );
    assert!(
        frame.is_keyframe(),
        "{width}x{height}: the encoder only emits key frames"
    );

    let luma = frame.plane(0).data();
    let w = width as usize;
    assert_eq!(luma.len(), w * height as usize, "{width}x{height}: Y size");

    let mut total = 0u64;
    for row in 0..height as usize {
        for col in 0..w {
            let want = i32::from(rgb[(row * w + col) * 3]);
            let got = i32::from(luma[row * w + col]);
            total += u64::from((want - got).unsigned_abs());
        }
    }
    total as f64 / f64::from(width * height)
}

/// `WebPLossyEncoder::encode_rgb` output must actually decode.
///
/// This used to be an "honest gap" test asserting the *failure* of this
/// round trip: `decode_vp8_keyframe` rejected every frame this encoder
/// produced, at every dimension, with
/// `InvalidBitstream("VP8: token partition exceeds payload")`. Nothing in
/// the tree had ever decoded the lossy encoder's output, so the encoder had
/// drifted into emitting a bitstream no conforming decoder could read (the
/// boolean coder dropped a byte and never propagated carries; the frame
/// header omitted `refresh_entropy_probs` and mis-coded the coefficient
/// probability-update gates; the mode and DCT token trees did not match RFC
/// 6386 §11/§13). See the `webp::encoder` module docs for the full list.
///
/// The bound below is deliberately generous: this asserts the encoder is
/// *correct*, not that it is good. Bit-exactness is not available — the
/// codec is lossy — and quality tuning is a separate concern. Measured mean
/// absolute luma error at quality 90 is ~0.7-1.6, so a bound of 8 leaves
/// wide headroom while still catching any regression that desynchronises
/// the range coder (which produces garbage, i.e. errors in the tens).
#[test]
fn test_encode_rgb_decodes_back_with_plausible_luma() {
    // Odd (5x3, sub-macroblock), non-macroblock-aligned (13x7), exactly one
    // macroblock (16x16), macroblock-aligned multi-MB (32x32), and a
    // multi-macroblock-row frame (64x48) whose later macroblocks predict
    // from earlier reconstructed ones — that last case is what catches an
    // encoder whose reconstruction disagrees with the decoder's, because
    // the prediction error compounds across the macroblock grid.
    for (width, height) in [(5u32, 3u32), (13, 7), (16, 16), (32, 32), (64, 48)] {
        let rgb = gray_gradient(width, height);
        let mae = encode_decode_luma_mae(width, height, &rgb);
        assert!(
            mae < 8.0,
            "{width}x{height}: decoded luma must track the source; mean absolute error {mae:.3} \
             is too large to be quantisation alone"
        );
    }
}

/// A two-tone image round-trips with the tone boundary intact.
///
/// A gradient can hide a constant-fill regression (its own mean is close to
/// every pixel); two flat halves cannot — if the encoder collapsed to a DC
/// average, one half would be off by half the tone separation.
#[test]
fn test_encode_rgb_two_tone_preserves_both_tones() {
    use oximedia_codec::webp::WebPLossyEncoder;

    let width = 32u32;
    let height = 32u32;
    let (dark, bright) = (40u8, 200u8);
    // The tone boundary sits at column 21, deliberately *inside* the second
    // macroblock column rather than on the 16-pixel grid: a boundary at
    // column 16 would leave every macroblock a single flat tone, which a
    // DC-only collapse would reproduce perfectly and which would never
    // exercise the AC token path at all.
    let boundary = 21usize;
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..height {
        for col in 0..width as usize {
            let value = if col < boundary { dark } else { bright };
            rgb.extend_from_slice(&[value, value, value]);
        }
    }

    let vp8_data = WebPLossyEncoder::new(90)
        .encode_rgb(&rgb, width, height)
        .expect("encode_rgb must succeed");
    let frame = decode_vp8_keyframe(&vp8_data).expect("two-tone frame must decode");
    let luma = frame.plane(0).data();
    let w = width as usize;

    // Sample well away from the boundary so ringing there is not what is
    // being measured.
    for row in 0..height as usize {
        let left = i32::from(luma[row * w + 2]);
        let right = i32::from(luma[row * w + w - 3]);
        assert!(
            (left - i32::from(dark)).abs() < 16,
            "row {row}: dark half decoded as {left}, expected about {dark}"
        );
        assert!(
            (right - i32::from(bright)).abs() < 16,
            "row {row}: bright half decoded as {right}, expected about {bright}"
        );
    }
}

/// The encoder's output must also survive the RIFF container hop, i.e. the
/// full `encode_rgb` -> `WebPWriter::write_lossy` -> `decode_vp8` path that
/// a caller writing a `.webp` file actually takes.
#[test]
fn test_encode_rgb_decodes_through_riff_container() {
    use oximedia_codec::webp::WebPLossyEncoder;

    let (width, height) = (32u32, 32u32);
    let rgb = gray_gradient(width, height);
    let vp8_data = WebPLossyEncoder::new(90)
        .encode_rgb(&rgb, width, height)
        .expect("encode_rgb must succeed");

    let webp = WebPWriter::write_lossy(&vp8_data);
    let via_container = decode_vp8(&webp).expect("encoder output must decode through RIFF");
    let via_raw = decode_vp8_keyframe(&vp8_data).expect("encoder output must decode raw");

    for i in 0..3 {
        assert_plane_bit_exact(
            &format!("encoded: container vs raw plane {i}"),
            via_container.plane(i).data(),
            via_raw.plane(i).data(),
        );
    }
}

/// Negative guard: a truncated encoder frame must still be rejected with a
/// typed error rather than decoded into a fabricated picture.
///
/// The truncation has to land *inside the first partition* to be detectable.
/// With a single token partition (which is what this encoder emits, RFC 6386
/// §9.5) the last partition is defined to run to the end of whatever payload
/// it is given, so lopping bytes off the token data alone is not a length
/// error — the decoder legitimately reads zeros past the end. Cutting below
/// `10 + first_partition_size` instead makes the frame tag's own declared
/// length overrun the buffer, which is a hard error.
#[test]
fn test_truncated_encoder_output_is_typed_error_not_panic() {
    use oximedia_codec::webp::WebPLossyEncoder;

    let (width, height) = (32u32, 32u32);
    let rgb = gray_gradient(width, height);
    let vp8_data = WebPLossyEncoder::new(90)
        .encode_rgb(&rgb, width, height)
        .expect("encode_rgb must succeed");
    assert!(
        decode_vp8_keyframe(&vp8_data).is_ok(),
        "baseline must decode"
    );

    // Frame tag bits 5..24 hold first_partition_size (RFC 6386 §9.1).
    let tag =
        u32::from(vp8_data[0]) | (u32::from(vp8_data[1]) << 8) | (u32::from(vp8_data[2]) << 16);
    let first_partition_size = ((tag >> 5) & 0x7_FFFF) as usize;
    let cut = 10 + first_partition_size - 1;
    assert!(cut > 10 && cut < vp8_data.len(), "truncation point sanity");

    let err = match decode_vp8_keyframe(&vp8_data[..cut]) {
        Err(e) => e,
        Ok(_) => panic!("a frame cut short of its own declared first partition must not decode"),
    };
    assert!(
        matches!(err, CodecError::InvalidBitstream(_)),
        "expected a typed InvalidBitstream error, not a panic or a fabricated frame: {err:?}"
    );
}

/// `WebPLossyEncoder::encode_rgb` does not reject odd width/height outright
/// (see also `webp::encoder`'s own `test_encode_rgb_non_mb_aligned` unit
/// test, which already exercises 7x5): the emitted VP8 frame tag declares
/// the true 5x3 dimensions, not a macroblock-padded size.
#[test]
fn test_encode_rgb_odd_dims_declares_true_dimensions() {
    use oximedia_codec::webp::WebPLossyEncoder;

    let width = 5u32;
    let height = 3u32;
    let rgb: Vec<u8> = (0..(width * height * 3))
        .map(|i| ((i * 37 + 11) % 256) as u8)
        .collect();

    let vp8_data = WebPLossyEncoder::new(90)
        .encode_rgb(&rgb, width, height)
        .expect("WebPLossyEncoder must accept odd width/height outright, not reject it");
    assert!(vp8_data.len() >= 10, "frame tag + sync code + dimensions");

    // The VP8 frame tag declares the true (unpadded) dimensions -- same
    // check `test_encode_rgb_non_mb_aligned` makes for 7x5 in encoder.rs.
    let declared_width = u16::from(vp8_data[6]) | (u16::from(vp8_data[7]) << 8);
    let declared_height = u16::from(vp8_data[8]) | (u16::from(vp8_data[9]) << 8);
    assert_eq!(declared_width & 0x3FFF, width as u16, "declared width");
    assert_eq!(declared_height & 0x3FFF, height as u16, "declared height");

    // ...and a decoder agrees, cropping to those dimensions.
    let frame = decode_vp8_keyframe(&vp8_data).expect("odd-dimension frame must decode");
    assert_eq!((frame.width, frame.height), (width, height));
}

/// A real, bit-valid odd-width lossy WebP through the public `ImageDecoder`
/// path, byte-compared against an independent `div_ceil` reference
/// conversion -- the test section 3 originally asked for, reached without
/// `WebPLossyEncoder` (see the honest-gap test above for why that route is
/// blocked today).
///
/// This takes the real `GRAD32_VP8` fixture (a spec-valid 32x32 VP8 key
/// frame, libwebp-encoded, already pinned bit-exact elsewhere in this
/// module) and patches only its declared-dimensions field (frame-tag bytes
/// 6-9, RFC 6386 §9.1) from 32x32 down to 31x31, leaving every coded
/// coefficient/mode/partition byte untouched. This stays a legal VP8 key
/// frame: the macroblock grid a decoder reconstructs is
/// `div_ceil(width, 16) x div_ceil(height, 16)`, and `div_ceil(31, 16) ==
/// div_ceil(32, 16) == 2` -- an unchanged 2x2 macroblock grid, so the
/// bitstream's actual coded data (which only ever describes whole
/// macroblocks) still parses exactly as before. Only the final crop to the
/// declared width/height changes, exactly the same way this crate's
/// `TEX48X40` fixture already proves for a non-macroblock-multiple
/// *height* (see `test_decode_vp8_tex48x40_bit_exact_through_riff_container`
/// above). The odd width also changes the *reported* chroma dimensions:
/// `div_ceil(31, 2) = 16`, identical to the true 32x32 case's chroma width,
/// so `decode_vp8_keyframe` hands back the same 16-wide chroma planes as
/// always -- but a floor-dividing reader would compute `31 / 2 = 15` and
/// read every row below the first with the wrong stride, exactly the
/// silent-shear bug this change fixes. Because `GRAD32` is a spatial
/// gradient (not a flat fill), a wrong stride produces real, non-degenerate
/// numeric mismatches rather than coincidentally-correct output.
#[cfg(feature = "image-io")]
#[test]
fn test_odd_width_real_bitstream_decodes_through_image_decoder_with_ceil_chroma() {
    use oximedia_codec::image::{yuv_to_rgb, ImageDecoder};

    let width = 31u32;
    let height = 31u32;
    let mut payload = fx::GRAD32_VP8.to_vec();
    payload[6] = 31; // declared width low byte (was 32); high byte/scale bits already 0
    payload[8] = 31; // declared height low byte (was 32); high byte/scale bits already 0

    let webp_container = WebPWriter::write_lossy(&payload);

    // Decode through the public ImageDecoder path -- this is the path that
    // silently sheared odd-width chroma before the image.rs fix.
    let decoded = ImageDecoder::new(&webp_container)
        .expect("a well-formed lossy WebP must be detected")
        .decode()
        .expect("odd-width lossy WebP must decode through ImageDecoder");
    assert_eq!(decoded.width, width);
    assert_eq!(decoded.height, height);
    assert_eq!(decoded.format, PixelFormat::Rgb24);
    assert_eq!(decoded.planes.len(), 1);
    let decoded_rgb = decoded.planes[0].data.clone();
    assert_eq!(decoded_rgb.len(), (width * height * 3) as usize);

    // Independent reference: decode the same raw (patched) VP8 payload
    // directly and convert YUV -> RGB in-test with an explicit div_ceil
    // chroma index and the same `yuv_to_rgb` free function
    // `image::convert_yuv420p_to_rgb` uses internally, so this is a
    // byte-exact cross-check rather than a second call into the code under
    // test.
    let yuv_frame =
        decode_vp8_keyframe(&payload).expect("decode_vp8_keyframe must decode the patched payload");
    assert_eq!(yuv_frame.width, width);
    assert_eq!(yuv_frame.height, height);
    let chroma_width = (width as usize).div_ceil(2);
    let chroma_height = (height as usize).div_ceil(2);
    assert_eq!(
        chroma_width, 16,
        "sanity check: ceil(31/2) must equal the true 32x32 case's chroma width"
    );
    assert_eq!(yuv_frame.planes[1].data.len(), chroma_width * chroma_height);
    assert_eq!(yuv_frame.planes[2].data.len(), chroma_width * chroma_height);

    let y_plane = &yuv_frame.planes[0].data;
    let u_plane = &yuv_frame.planes[1].data;
    let v_plane = &yuv_frame.planes[2].data;
    let w = width as usize;
    let h = height as usize;
    let mut reference_rgb = vec![0u8; w * h * 3];
    for row in 0..h {
        for col in 0..w {
            let y_val = y_plane[row * w + col];
            let chroma_idx = (row / 2) * chroma_width + (col / 2);
            let u_val = u_plane[chroma_idx];
            let v_val = v_plane[chroma_idx];
            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val);
            let out = (row * w + col) * 3;
            reference_rgb[out] = r;
            reference_rgb[out + 1] = g;
            reference_rgb[out + 2] = b;
        }
    }

    assert_eq!(
        decoded_rgb, reference_rgb,
        "ImageDecoder's odd-width lossy-WebP RGB output must byte-match an \
         independent div_ceil chroma-index reference conversion of the same \
         decoded YUV planes"
    );
}

/// Encodes a solid-colour frame and returns the decoded chroma plane means
/// `(mean_u, mean_v)`.
///
/// A solid colour is the friendliest possible input for a DC-prediction-only
/// encoder — every macroblock is flat — so the decoded chroma means should
/// sit within a few code values of the BT.601 conversion of the source
/// colour. What this measures is therefore plane *identity and sign*, not
/// codec quality.
fn encode_decode_chroma_means(r: u8, g: u8, b: u8) -> (f64, f64) {
    use oximedia_codec::webp::WebPLossyEncoder;

    const W: u32 = 32;
    const H: u32 = 32;
    let mut rgb = Vec::with_capacity((W * H * 3) as usize);
    for _ in 0..W * H {
        rgb.extend_from_slice(&[r, g, b]);
    }

    let vp8_data = WebPLossyEncoder::new(90)
        .encode_rgb(&rgb, W, H)
        .unwrap_or_else(|e| panic!("solid ({r},{g},{b}): encode_rgb must succeed, got {e}"));
    let frame = decode_vp8_keyframe(&vp8_data)
        .unwrap_or_else(|e| panic!("solid ({r},{g},{b}): decode must succeed, got {e}"));

    let mean = |plane: usize| {
        let data = frame.plane(plane).data();
        assert!(!data.is_empty(), "plane {plane} must not be empty");
        data.iter().map(|&v| f64::from(v)).sum::<f64>() / data.len() as f64
    };
    (mean(1), mean(2))
}

/// The luma round-trip tests all use `R == G == B`, which pins U and V to
/// 128 and therefore cannot see a swapped U/V plane, an inverted chroma
/// sign, or broken chroma DC coding. Solid red and solid blue discriminate
/// all three: in BT.601, red drives Cr (the V plane) far above 128 while
/// pulling Cb (U) below it, and blue does exactly the opposite.
#[test]
fn test_encode_rgb_chroma_planes_carry_colour_with_correct_identity() {
    let (red_u, red_v) = encode_decode_chroma_means(255, 0, 0);
    let (blue_u, blue_v) = encode_decode_chroma_means(0, 0, 255);

    // Absolute positions: BT.601 puts solid red near (U, V) = (90, 240) and
    // solid blue near (240, 110). Generous +/- windows keep this a plane
    // -identity assertion, not a quality bound.
    assert!(
        red_v > 180.0 && red_u < 110.0,
        "solid red must land high-V / low-U, got U {red_u:.1}, V {red_v:.1} \
         (a swapped U/V plane or inverted Cr sign lands here)"
    );
    assert!(
        blue_u > 180.0 && blue_v < 128.0,
        "solid blue must land high-U / low-V, got U {blue_u:.1}, V {blue_v:.1}"
    );

    // Relative discrimination, immune to any shared offset drift.
    assert!(
        red_v > blue_v + 60.0 && blue_u > red_u + 60.0,
        "red and blue must separate on both chroma axes: red (U {red_u:.1}, \
         V {red_v:.1}) vs blue (U {blue_u:.1}, V {blue_v:.1})"
    );
}
