//! Proves that `oximedia_videoip::codec`'s compressed video decoders really
//! reconstruct pixels, and that every codec direction without a working
//! implementation fails honestly instead of fabricating output.
//!
//! The three bitstreams under `tests/fixtures/` were encoded by libvpx and
//! libaom (via FFmpeg 7.1.1), not by OxiMedia — see `tests/fixtures/README.md`
//! for the exact commands and for where the expected pixel values come from.

use oximedia_videoip::codec::{create_audio_encoder, create_video_decoder, create_video_encoder};
use oximedia_videoip::error::VideoIpError;
use oximedia_videoip::types::{AudioCodec, VideoCodec};

/// Geometry of every fixture. Nothing tells the decoder this — it has to come
/// out of the bitstream, which is exactly what the assertions check.
const WIDTH: usize = 128;
const HEIGHT: usize = 96;
const Y_SIZE: usize = WIDTH * HEIGHT;
const C_WIDTH: usize = WIDTH / 2;
const C_SIZE: usize = C_WIDTH * (HEIGHT / 2);
const I420_SIZE: usize = Y_SIZE + 2 * C_SIZE;

const VP8_KEYFRAME: &[u8] = include_bytes!("fixtures/vp8_kf.bin");
const VP9_KEYFRAME: &[u8] = include_bytes!("fixtures/vp9_kf.bin");
const AV1_KEYFRAME: &[u8] = include_bytes!("fixtures/av1_kf.bin");

/// `(x, y, expected)` samples taken from the FFmpeg reference reconstruction.
///
/// They are deliberately away from row 0 and column 0: a stride-vs-width
/// mix-up in the plane packing shifts every later row and is caught here,
/// where a length-only check would pass.
struct Expected {
    luma: &'static [(usize, usize, u8)],
    cb: &'static [(usize, usize, u8)],
    cr: &'static [(usize, usize, u8)],
}

fn check_decode(codec: VideoCodec, bitstream: &[u8], expected: &Expected) {
    // `None`: no resolution is supplied, so the geometry below can only have
    // come from the bitstream itself.
    let mut decoder =
        create_video_decoder(codec, None).unwrap_or_else(|e| panic!("{codec:?} decoder: {e}"));

    let frame = decoder
        .decode(bitstream, 1_234_567, false)
        .unwrap_or_else(|e| panic!("{codec:?} decode: {e}"))
        .unwrap_or_else(|| panic!("{codec:?}: keyframe produced no frame"));

    assert_eq!(frame.width, WIDTH as u32, "{codec:?} width from bitstream");
    assert_eq!(
        frame.height, HEIGHT as u32,
        "{codec:?} height from bitstream"
    );
    assert_eq!(
        frame.data.len(),
        I420_SIZE,
        "{codec:?} payload must be tightly packed I420"
    );
    assert_eq!(frame.pts, 1_234_567, "{codec:?} pts must be preserved");
    assert!(
        frame.is_keyframe,
        "{codec:?}: bitstream says keyframe, even though the packet flag said otherwise"
    );

    // A zero-filled or constant buffer would satisfy every check above.
    let distinct: std::collections::BTreeSet<u8> = frame.data[..Y_SIZE].iter().copied().collect();
    assert!(
        distinct.len() > 32,
        "{codec:?}: only {} distinct luma values -- that is not a decoded picture",
        distinct.len()
    );

    for &(x, y, want) in expected.luma {
        assert_eq!(
            frame.data[y * WIDTH + x],
            want,
            "{codec:?} luma at ({x},{y})"
        );
    }
    for &(x, y, want) in expected.cb {
        assert_eq!(
            frame.data[Y_SIZE + y * C_WIDTH + x],
            want,
            "{codec:?} Cb at ({x},{y})"
        );
    }
    for &(x, y, want) in expected.cr {
        assert_eq!(
            frame.data[Y_SIZE + C_SIZE + y * C_WIDTH + x],
            want,
            "{codec:?} Cr at ({x},{y})"
        );
    }
}

#[test]
fn vp8_keyframe_decodes_to_the_reference_picture() {
    check_decode(
        VideoCodec::Vp8,
        VP8_KEYFRAME,
        &Expected {
            luma: &[(10, 7, 147), (64, 48, 41), (100, 80, 106), (127, 95, 170)],
            cb: &[(5, 3, 97), (32, 24, 241), (60, 44, 166)],
            cr: &[(5, 3, 159), (32, 24, 109), (60, 44, 16)],
        },
    );
}

#[test]
fn vp9_keyframe_decodes_to_the_reference_picture() {
    check_decode(
        VideoCodec::Vp9,
        VP9_KEYFRAME,
        &Expected {
            luma: &[(10, 7, 146), (64, 48, 41), (100, 80, 106), (127, 95, 170)],
            cb: &[(5, 3, 97), (32, 24, 240), (60, 44, 166)],
            cr: &[(5, 3, 157), (32, 24, 111), (60, 44, 16)],
        },
    );
}

#[test]
fn av1_keyframe_decodes_to_the_reference_picture() {
    check_decode(
        VideoCodec::Av1,
        AV1_KEYFRAME,
        &Expected {
            luma: &[(10, 7, 143), (64, 48, 40), (100, 80, 106), (127, 95, 170)],
            cb: &[(5, 3, 93), (32, 24, 240), (60, 44, 166)],
            cr: &[(5, 3, 162), (32, 24, 109), (60, 44, 16)],
        },
    );
}

/// A decoder that answered `Ok` for input it cannot parse would be the same
/// defect this slice removed; garbage must be reported.
#[test]
fn compressed_decoders_reject_garbage() {
    for codec in [VideoCodec::Vp8, VideoCodec::Vp9, VideoCodec::Av1] {
        let mut decoder =
            create_video_decoder(codec, None).unwrap_or_else(|e| panic!("{codec:?}: {e}"));
        let outcome = decoder.decode(&[0xAB; 64], 0, true);
        assert!(
            !matches!(outcome, Ok(Some(_))),
            "{codec:?} produced a frame from 64 garbage bytes"
        );
    }
}

/// The other half of the contract: nothing claims to compress.
#[test]
fn no_compressed_encoder_is_offered() {
    for codec in [VideoCodec::Vp8, VideoCodec::Vp9, VideoCodec::Av1] {
        let err = create_video_encoder(codec, 128, 96, Some(2_000_000))
            .err()
            .unwrap_or_else(|| panic!("{codec:?} must not offer an encoder"));
        assert!(
            matches!(
                err,
                VideoIpError::CodecUnimplemented {
                    operation: "encode",
                    ..
                }
            ),
            "{codec:?}: unexpected error {err}"
        );
    }

    let err = create_audio_encoder(AudioCodec::Opus, 48000, 2)
        .err()
        .expect("Opus must not offer an encoder");
    assert!(matches!(
        err,
        VideoIpError::CodecUnimplemented {
            codec: "Opus",
            operation: "encode",
            ..
        }
    ));
}
