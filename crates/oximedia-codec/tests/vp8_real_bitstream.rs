//! Real-bitstream VP8 key-frame decode tests.
//!
//! These tests feed genuine VP8 key frames produced by the reference
//! encoders (libwebp's `cwebp` and libvpx via `ffmpeg`) into
//! [`Vp8Decoder`] and verify the decoded pixels two ways:
//!
//! 1. **Bit-exactness**: every decoded Y/U/V plane must match the
//!    reference reconstruction produced by libwebp's `dwebp -yuv` for the
//!    same bitstream, byte for byte. VP8 decoding is exactly specified
//!    (RFC 6386), so any deviation is a decoder bug.
//! 2. **Source fidelity (PSNR)**: the decoded luma must be close to the
//!    pre-encode source luma. The libwebp reference decode measures
//!    ~33.0 dB (grad32) and ~34.7 dB (tex48x40) against the source, so a
//!    >= 30 dB bound proves real, faithful pixels rather than noise or a
//!    constant fill.
//!
//! A REAL libvpx inter frame (frame 2 of the IVF stream) must produce the
//! honest `UnsupportedFeature` error — never a blank frame.

#![cfg(feature = "vp8")]

mod vp8_fixtures;

use oximedia_codec::error::CodecError;
use oximedia_codec::frame::VideoFrame;
use oximedia_codec::traits::{DecoderConfig, VideoDecoder};
use oximedia_codec::vp8::Vp8Decoder;
use oximedia_core::PixelFormat;
use vp8_fixtures as fx;

/// Decodes a single VP8 key-frame payload and returns the emitted frame.
fn decode_keyframe(payload: &[u8]) -> VideoFrame {
    let mut decoder = Vp8Decoder::new(DecoderConfig::default()).expect("decoder construction");
    decoder
        .send_packet(payload, 0)
        .expect("key frame must decode");
    decoder
        .receive_frame()
        .expect("receive_frame must not error")
        .expect("a shown key frame must emit exactly one frame")
}

/// Peak signal-to-noise ratio between two equally-sized 8-bit planes.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "psnr operands must match in size");
    let mse: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let d = f64::from(i32::from(x) - i32::from(y));
            d * d
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// Asserts a decoded plane equals the `dwebp -yuv` reference bit-exactly,
/// with a diagnostic that pinpoints the first mismatch.
fn assert_plane_bit_exact(name: &str, got: &[u8], want: &[u8], width: usize) {
    assert_eq!(
        got.len(),
        want.len(),
        "{name}: plane size mismatch (got {}, want {})",
        got.len(),
        want.len()
    );
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        panic!(
            "{name}: first mismatch at index {i} (x={}, y={}): got {}, want {} \
             (plane PSNR vs reference: {:.2} dB)",
            i % width,
            i / width,
            got[i],
            want[i],
            psnr(got, want),
        );
    }
}

/// Full verification of one key-frame fixture against the libwebp
/// reference planes and the source-luma PSNR bound.
fn verify_keyframe_fixture(
    tag: &str,
    payload: &[u8],
    width: u32,
    height: u32,
    expected_y: &[u8],
    expected_u: &[u8],
    expected_v: &[u8],
    source_y: Option<&[u8]>,
) {
    let frame = decode_keyframe(payload);

    // Frame metadata.
    assert_eq!(frame.width, width, "{tag}: width");
    assert_eq!(frame.height, height, "{tag}: height");
    assert_eq!(frame.format, PixelFormat::Yuv420p, "{tag}: pixel format");
    assert!(frame.is_keyframe(), "{tag}: must be marked as a key frame");
    assert_eq!(frame.planes.len(), 3, "{tag}: YUV 4:2:0 has three planes");

    let w = width as usize;
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    assert_eq!(
        frame.plane(0).data().len(),
        w * height as usize,
        "{tag}: Y size"
    );
    assert_eq!(frame.plane(1).data().len(), cw * ch, "{tag}: U size");
    assert_eq!(frame.plane(2).data().len(), cw * ch, "{tag}: V size");

    // 1. Bit-exact against the libwebp (dwebp -yuv) reference reconstruction.
    assert_plane_bit_exact(&format!("{tag}.Y"), frame.plane(0).data(), expected_y, w);
    assert_plane_bit_exact(&format!("{tag}.U"), frame.plane(1).data(), expected_u, cw);
    assert_plane_bit_exact(&format!("{tag}.V"), frame.plane(2).data(), expected_v, cw);

    // 2. Decode quality against the pre-encode source luma.
    if let Some(src_y) = source_y {
        let db = psnr(frame.plane(0).data(), src_y);
        assert!(
            db >= 30.0,
            "{tag}: decoded luma PSNR vs source must be >= 30 dB, got {db:.2} dB"
        );
    }
}

#[test]
fn test_decode_libwebp_keyframe_gradient_32x32_bit_exact_and_psnr() {
    verify_keyframe_fixture(
        "grad32",
        &fx::GRAD32_VP8,
        32,
        32,
        &fx::GRAD32_EXPECTED_Y,
        &fx::GRAD32_EXPECTED_U,
        &fx::GRAD32_EXPECTED_V,
        Some(&fx::GRAD32_SOURCE_Y),
    );
}

#[test]
fn test_decode_libwebp_keyframe_textured_48x40_bit_exact_and_psnr() {
    // 48x40: height is not a macroblock multiple, so this also proves the
    // bottom macroblock row is decoded and cropped correctly.
    verify_keyframe_fixture(
        "tex48x40",
        &fx::TEX48X40_VP8,
        48,
        40,
        &fx::TEX48X40_EXPECTED_Y,
        &fx::TEX48X40_EXPECTED_U,
        &fx::TEX48X40_EXPECTED_V,
        Some(&fx::TEX48X40_SOURCE_Y),
    );
}

#[test]
fn test_decode_libvpx_keyframe_48x48_multi_partition_bit_exact() {
    // Encoded by libvpx with `-slices 4`: the key frame carries four DCT
    // token partitions, exercising the multi-partition setup path.
    verify_keyframe_fixture(
        "vpx_kf",
        &fx::VPX_KEYFRAME_VP8,
        48,
        48,
        &fx::VPX_KF_EXPECTED_Y,
        &fx::VPX_KF_EXPECTED_U,
        &fx::VPX_KF_EXPECTED_V,
        None,
    );
}

#[test]
fn test_real_libvpx_inter_frame_returns_honest_unsupported_error() {
    let mut decoder = Vp8Decoder::new(DecoderConfig::default()).expect("decoder construction");

    // Frame 1 of the IVF stream: the key frame decodes normally.
    decoder
        .send_packet(&fx::VPX_KEYFRAME_VP8, 0)
        .expect("key frame must decode");
    assert!(
        decoder
            .receive_frame()
            .expect("receive_frame must not error")
            .is_some(),
        "key frame must be emitted"
    );

    // Frame 2: a REAL libvpx inter frame. Inter decoding (motion vectors,
    // golden/altref references) is not implemented -> honest error.
    let err = match decoder.send_packet(&fx::VPX_INTER_FRAME_VP8, 1) {
        Err(e) => e,
        Ok(()) => panic!("a real inter frame must not decode yet"),
    };
    assert!(
        matches!(err, CodecError::UnsupportedFeature(_)),
        "expected UnsupportedFeature for an inter frame, got {err:?}"
    );
    assert!(
        err.to_string()
            .contains("inter-frame decode not yet implemented"),
        "error must name the gap, got: {err}"
    );

    // And crucially: no blank frame may be fabricated for the inter frame.
    assert!(
        decoder
            .receive_frame()
            .expect("receive_frame must not error")
            .is_none(),
        "no blank frame may be emitted for an unsupported inter frame"
    );
}

#[test]
fn test_decoded_keyframe_is_not_a_constant_fill() {
    // Guards against any regression to fabricated constant-gray output:
    // the decoded planes must carry real variation.
    let frame = decode_keyframe(&fx::TEX48X40_VP8);
    for (idx, name) in [(0usize, "Y"), (1, "U"), (2, "V")] {
        let data = frame.plane(idx).data();
        let min = data.iter().copied().min().unwrap_or(0);
        let max = data.iter().copied().max().unwrap_or(0);
        assert!(
            max - min > 8,
            "{name} plane looks like a constant fill (min {min}, max {max})"
        );
    }
}

#[test]
fn test_stream_properties_after_real_keyframe() {
    let mut decoder = Vp8Decoder::new(DecoderConfig::default()).expect("decoder construction");
    assert!(decoder.dimensions().is_none());
    decoder
        .send_packet(&fx::GRAD32_VP8, 0)
        .expect("key frame must decode");
    assert_eq!(decoder.dimensions(), Some((32, 32)));
    assert_eq!(decoder.output_format(), Some(PixelFormat::Yuv420p));
    let frame = decoder
        .receive_frame()
        .expect("receive_frame must not error")
        .expect("frame must be emitted");
    assert_eq!(frame.timestamp.pts, 0);
}
