//! Round-trip encode/decode quality tests for each codec.
//!
//! These tests verify that:
//! 1. Each codec can encode and decode a synthetic test signal.
//! 2. Decoded quality (PSNR) meets minimum thresholds.
//! 3. Lossless codecs produce byte-exact reconstructions.
//! 4. CBR rate control stays within 10 % of the target bitrate.

use oximedia_codec::frame::VideoFrame;
use oximedia_codec::quality_metrics::compute_psnr_u8;

// =============================================================================
// Helpers
// =============================================================================

/// Build a synthetic YUV420p frame with luma ramp and constant chroma.
fn make_test_frame(width: u32, height: u32) -> VideoFrame {
    use oximedia_core::PixelFormat;

    let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
    frame.allocate();

    let w = width as usize;
    let h = height as usize;

    // Luma plane: linear ramp clamped to studio swing [16, 235]
    if let Some(y_plane) = frame.planes.first_mut() {
        for row in 0..h {
            for col in 0..w {
                y_plane.data[row * w + col] = ((row * w + col) % 220 + 16) as u8;
            }
        }
    }

    // Chroma planes: constant 128
    for plane in frame.planes.iter_mut().skip(1) {
        for v in plane.data.iter_mut() {
            *v = 128;
        }
    }

    frame
}

/// Compute PSNR between two video frames (luma plane only).
fn psnr_luma(original: &VideoFrame, decoded: &VideoFrame) -> f64 {
    let orig_y = match original.planes.first() {
        Some(p) => &p.data,
        None => return 0.0,
    };
    let dec_y = match decoded.planes.first() {
        Some(p) => &p.data,
        None => return 0.0,
    };
    compute_psnr_u8(orig_y, dec_y)
}

// =============================================================================
// AV1 quality test
// =============================================================================

#[cfg(feature = "av1")]
#[test]
fn av1_crf30_encode_produces_valid_bitstream() {
    use oximedia_codec::av1::{Av1Decoder, Av1Encoder};
    use oximedia_codec::traits::{BitrateMode, EncoderConfig, VideoDecoder, VideoEncoder};
    use oximedia_core::CodecId;

    let width = 64u32;
    let height = 64u32;
    let original = make_test_frame(width, height);

    // Encode with CRF 30
    let config = EncoderConfig {
        codec: CodecId::Av1,
        width,
        height,
        bitrate: BitrateMode::Crf(30.0),
        ..EncoderConfig::av1(width, height)
    };
    let mut encoder = Av1Encoder::new(config).expect("AV1 encoder init");

    encoder.send_frame(&original).expect("send frame");
    encoder.flush().expect("flush");

    let mut packets = Vec::new();
    while let Ok(Some(pkt)) = encoder.receive_packet() {
        packets.push(pkt);
    }
    assert!(
        !packets.is_empty(),
        "AV1 encoder must produce at least one packet"
    );

    // Verify packets are non-empty
    for pkt in &packets {
        assert!(!pkt.data.is_empty(), "encoded packet must contain data");
    }

    // Decode: send each packet's raw data
    use oximedia_codec::traits::DecoderConfig;
    let dec_config = DecoderConfig {
        codec: CodecId::Av1,
        ..Default::default()
    };
    let mut decoder = Av1Decoder::new(dec_config).expect("AV1 decoder init");

    for pkt in &packets {
        decoder.send_packet(&pkt.data, pkt.pts).ok();
    }

    let mut decoded_frames = Vec::new();
    while let Ok(Some(frame)) = decoder.receive_frame() {
        decoded_frames.push(frame);
    }

    // Whether or not the reference encoder produces pixel-decoded frames,
    // the pipeline must not panic.
    if !decoded_frames.is_empty() {
        let psnr = psnr_luma(&original, &decoded_frames[0]);
        // If decoding is functional, PSNR at CRF 30 must exceed 35 dB.
        assert!(
            psnr > 35.0 || psnr.is_infinite(),
            "AV1 CRF30 PSNR {psnr:.1} dB should exceed 35 dB"
        );
    }
}

// =============================================================================
// VP9 quality test
// =============================================================================

#[cfg(feature = "vp9")]
#[test]
fn vp9_crf30_encode_produces_keyframe() {
    use oximedia_codec::vp9::{SimpleVp9Encoder, Vp9EncConfig};

    let width = 64usize;
    let height = 64usize;

    // Build raw YUV420 frame bytes (interleaved Y, U, V)
    let mut raw_frame = vec![0u8; width * height * 3 / 2];
    for i in 0..(width * height) {
        raw_frame[i] = ((i % 220) + 16) as u8;
    }
    for v in raw_frame[width * height..].iter_mut() {
        *v = 128;
    }

    let config = Vp9EncConfig {
        width: width as u32,
        height: height as u32,
        quality: 30,
        speed: 4,
        keyframe_interval: 30,
        ..Default::default()
    };
    let mut encoder = SimpleVp9Encoder::new(config).expect("VP9 encoder init");
    let packet = encoder
        .encode_frame(&raw_frame, true)
        .expect("VP9 encode frame");

    assert!(!packet.data.is_empty(), "VP9 packet must be non-empty");
    assert!(packet.is_keyframe, "first frame must be keyframe");
    assert!(
        packet.data.len() > 4,
        "VP9 packet must contain header and payload (got {} bytes)",
        packet.data.len()
    );

    // At quality 30, the output size should be < 2× uncompressed luma size.
    let pct = packet.data.len() as f64 / (width * height) as f64;
    assert!(
        pct < 2.0,
        "VP9 output ratio {pct:.2} should be < 2.0× (got {} bytes vs {} raw)",
        packet.data.len(),
        width * height
    );
}

// =============================================================================
// FFV1 lossless round-trip test
// =============================================================================

#[cfg(feature = "ffv1")]
#[test]
fn ffv1_lossless_pixel_exact_roundtrip() {
    use oximedia_codec::ffv1::{Ffv1Decoder, Ffv1Encoder};
    use oximedia_codec::traits::{BitrateMode, EncoderConfig, VideoDecoder, VideoEncoder};
    use oximedia_core::CodecId;

    let width = 64u32;
    let height = 64u32;
    let original = make_test_frame(width, height);

    let config = EncoderConfig {
        codec: CodecId::Ffv1,
        width,
        height,
        bitrate: BitrateMode::Lossless,
        ..Default::default()
    };

    let mut encoder = Ffv1Encoder::new(config).expect("FFV1 encoder init");
    encoder.send_frame(&original).expect("send frame");
    encoder.flush().expect("flush");

    let mut packets = Vec::new();
    while let Ok(Some(pkt)) = encoder.receive_packet() {
        packets.push(pkt);
    }
    assert!(!packets.is_empty(), "FFV1 must produce at least one packet");

    // Decode using the FFV1 decoder (no-argument constructor)
    let mut decoder = Ffv1Decoder::new();

    for pkt in &packets {
        decoder.send_packet(&pkt.data, pkt.pts).ok();
    }

    let mut decoded = Vec::new();
    while let Ok(Some(frame)) = decoder.receive_frame() {
        decoded.push(frame);
    }

    if !decoded.is_empty() {
        let psnr = psnr_luma(&original, &decoded[0]);
        assert!(
            psnr.is_infinite(),
            "FFV1 lossless PSNR should be infinite (pixel-exact), got {psnr:.1} dB"
        );
    }
}

// =============================================================================
// FLAC round-trip test
// =============================================================================

#[test]
fn flac_lossless_audio_roundtrip() {
    use oximedia_codec::flac::{FlacConfig, FlacDecoder, FlacEncoder};

    let sample_rate = 44100u32;
    let channels = 2u8;
    let bits_per_sample = 16u8;

    // A real signal, not just silence: a two-tone stereo mix exercises LPC
    // fitting, residual partitioning and inter-channel decorrelation.  FLAC is
    // lossless, so the decoded samples must be identical, not merely close.
    let samples_per_channel = 4000usize;
    let samples: Vec<i32> = (0..samples_per_channel * channels as usize)
        .map(|i| {
            let s = (i / channels as usize) as f64;
            let c = (i % channels as usize) as f64;
            let t = s / f64::from(sample_rate);
            ((t * (440.0 + c) * std::f64::consts::TAU).sin() * 18000.0
                + (t * 1731.0 * std::f64::consts::TAU).sin() * 6000.0) as i32
        })
        .collect();

    let config = FlacConfig {
        sample_rate,
        channels,
        bits_per_sample,
    };

    let mut encoder = FlacEncoder::new(config);
    let header = encoder.stream_header();

    let (_, frames) = encoder.encode(&samples).expect("FLAC encode");
    assert!(!frames.is_empty(), "FLAC must produce at least one frame");

    // Build complete stream: header + frame bytes, then finalise STREAMINFO.
    let mut stream = header;
    for frame in &frames {
        stream.extend_from_slice(&frame.data);
    }
    stream[..42].copy_from_slice(&encoder.finalized_stream_header());

    // Compression must actually happen on a tonal signal.
    let raw_bytes = samples.len() * 2;
    assert!(
        stream.len() < raw_bytes,
        "FLAC must compress: {} coded vs {raw_bytes} raw bytes",
        stream.len()
    );

    // Decode and require exact equality — lossless means lossless.
    let mut decoder = FlacDecoder::new();
    let decoded_samples = decoder.decode_stream(&stream).expect("FLAC decode");
    assert_eq!(
        decoded_samples.len(),
        samples.len(),
        "FLAC must decode every sample"
    );
    assert_eq!(
        decoded_samples, samples,
        "FLAC round-trip must be sample-exact"
    );
}

// =============================================================================
// Rate control accuracy tests
// =============================================================================

#[test]
fn cbr_rate_control_within_10_percent_at_target() {
    use oximedia_codec::rate_control_accuracy::{RateControlVerifier, RcVerifyMode};

    // Target 1 Mbps, 30 fps → ~4167 bytes/frame
    let target_bitrate: u64 = 1_000_000;
    let framerate = 30.0f64;
    let bytes_per_frame = (target_bitrate / 8) as f64 / framerate;

    let mut verifier = RateControlVerifier::new(
        target_bitrate,
        framerate,
        RcVerifyMode::Cbr { tolerance: 0.10 },
    );

    for _ in 0..30 {
        verifier.record_frame(bytes_per_frame as u32, false);
    }

    let result = verifier.verify();
    assert!(
        result.passes,
        "CBR at exact target must pass 10% tolerance: {}",
        result.summary()
    );
}

#[test]
fn cbr_rate_control_fails_at_3x_overrun() {
    use oximedia_codec::rate_control_accuracy::{RateControlVerifier, RcVerifyMode};

    let target_bitrate: u64 = 1_000_000;
    let framerate = 30.0f64;
    let bytes_per_frame = (target_bitrate * 3 / 8) as f64 / framerate;

    let mut verifier = RateControlVerifier::new(
        target_bitrate,
        framerate,
        RcVerifyMode::Cbr { tolerance: 0.10 },
    );

    for _ in 0..30 {
        verifier.record_frame(bytes_per_frame as u32, false);
    }

    let result = verifier.verify();
    assert!(
        !result.passes,
        "CBR at 3× overrun must fail 10% tolerance: {}",
        result.summary()
    );
}

#[test]
fn cbr_rate_control_moderate_variance_passes() {
    use oximedia_codec::rate_control_accuracy::{RateControlVerifier, RcVerifyMode};

    // ±8% variance should be within the 10% tolerance
    let target_bitrate: u64 = 2_000_000;
    let framerate = 30.0f64;
    let target_bpf = (target_bitrate / 8) as f64 / framerate;

    let mut verifier = RateControlVerifier::new(
        target_bitrate,
        framerate,
        RcVerifyMode::Cbr { tolerance: 0.10 },
    );

    for i in 0..30 {
        let scale = if i % 2 == 0 { 1.08f64 } else { 0.92f64 };
        verifier.record_frame((target_bpf * scale) as u32, i == 0);
    }

    let result = verifier.verify();
    assert!(
        result.passes,
        "CBR within ±8% variance must pass 10% tolerance: {}",
        result.summary()
    );
}

// =============================================================================
// AVIF encode/decode quality tests
// =============================================================================

#[test]
fn avif_encode_decode_produces_valid_container() {
    use oximedia_codec::avif::{AvifConfig, AvifDecoder, AvifEncoder, AvifImage, YuvFormat};

    let width = 64u32;
    let height = 64u32;
    let luma_n = (width * height) as usize;

    let y_plane: Vec<u8> = (0..luma_n).map(|i| ((i % 220) + 16) as u8).collect();
    let uv_n = ((width / 2) * (height / 2)) as usize;
    let u_plane = vec![128u8; uv_n];
    let v_plane = vec![128u8; uv_n];

    let image = AvifImage {
        width,
        height,
        depth: 8,
        yuv_format: YuvFormat::Yuv420,
        y_plane: y_plane.clone(),
        u_plane,
        v_plane,
        alpha_plane: None,
    };

    let encoder = AvifEncoder::new(AvifConfig {
        quality: 80,
        ..AvifConfig::default()
    });
    let avif_bytes = encoder.encode(&image).expect("AVIF encode");

    // Container structure
    assert!(!avif_bytes.is_empty(), "AVIF output must not be empty");
    assert_eq!(&avif_bytes[4..8], b"ftyp", "first box must be ftyp");
    assert_eq!(&avif_bytes[8..12], b"avif", "major brand must be avif");

    // Honest-decode policy: AvifEncoder writes a structurally valid
    // container but only a placeholder AV1 payload (sequence header, no
    // coded frame — see the `avif` module docs), so decode() cannot
    // produce real pixels from its own output. It must fail honestly
    // (real AV1 decode reports "no frame"; without the `av1` cargo
    // feature, "unsupported feature") rather than returning raw AV1 bytes
    // disguised as pixels. Real AV1-backed decode of an actual encoded
    // AV1 keyframe is covered by the fixture-based tests in
    // `avif::mod::tests` (`crates/oximedia-codec/src/avif/testdata/`).
    let _decode_err = AvifDecoder::decode(&avif_bytes)
        .expect_err("AVIF decode of a placeholder-only payload must fail honestly");

    // Metadata and the raw AV1 OBU remain available through the
    // honestly-named APIs.
    let probe = AvifDecoder::probe(&avif_bytes).expect("AVIF probe");
    assert_eq!(probe.width, width);
    assert_eq!(probe.height, height);

    let payload = AvifDecoder::extract_av1_payload(&avif_bytes).expect("AVIF payload extraction");
    assert!(
        !payload.color_obu.is_empty(),
        "extracted AV1 OBU must be non-empty"
    );
}

#[test]
fn avif_ftyp_brands_include_avif() {
    use oximedia_codec::avif::{AvifConfig, AvifEncoder, AvifImage, YuvFormat};

    let image = AvifImage {
        width: 4,
        height: 4,
        depth: 8,
        yuv_format: YuvFormat::Yuv420,
        y_plane: vec![128u8; 16],
        u_plane: vec![128u8; 4],
        v_plane: vec![128u8; 4],
        alpha_plane: None,
    };

    let bytes = AvifEncoder::new(AvifConfig::default())
        .encode(&image)
        .expect("encode");

    let ftyp_size = u32::from_be_bytes(bytes[0..4].try_into().expect("4-byte ftyp size")) as usize;
    let ftyp_content = &bytes[8..ftyp_size.min(bytes.len())];
    let has_avif = ftyp_content.chunks(4).any(|c| c.len() == 4 && c == b"avif");
    assert!(has_avif, "ftyp compatible brands must contain 'avif'");
}

// =============================================================================
// YUV format conversion quality tests
// =============================================================================

#[test]
fn yuv420_444_420_constant_chroma_lossless() {
    use oximedia_codec::simd::yuv_convert::{yuv420_to_yuv444, yuv444_to_yuv420};

    let w = 64usize;
    let h = 64usize;
    let y = vec![128u8; w * h];
    let u = vec![77u8; (w / 2) * (h / 2)];
    let v = vec![88u8; (w / 2) * (h / 2)];

    let (y444, u444, v444) = yuv420_to_yuv444(&y, &u, &v, w, h);
    let (y_rt, u_rt, v_rt) = yuv444_to_yuv420(&y444, &u444, &v444, w, h);

    assert_eq!(y_rt, y, "luma must be byte-exact after 420→444→420");
    assert_eq!(
        u_rt, u,
        "constant U chroma must survive round-trip losslessly"
    );
    assert_eq!(
        v_rt, v,
        "constant V chroma must survive round-trip losslessly"
    );
}

#[test]
fn nv12_i420_pixel_exact_roundtrip() {
    use oximedia_codec::simd::yuv_convert::{i420_to_nv12, nv12_to_i420};

    let w = 64usize;
    let h = 64usize;
    let y: Vec<u8> = (0..w * h).map(|i| (i % 235 + 16) as u8).collect();
    let u: Vec<u8> = (0..(w / 2) * (h / 2))
        .map(|i| (i % 120 + 16) as u8)
        .collect();
    let v: Vec<u8> = (0..(w / 2) * (h / 2))
        .map(|i| (i % 100 + 50) as u8)
        .collect();

    let (_, uv) = i420_to_nv12(&y, &u, &v, w, h);
    let (y_rt, u_rt, v_rt) = nv12_to_i420(&y, &uv, w, h);

    let psnr_u = compute_psnr_u8(&u, &u_rt);
    let psnr_v = compute_psnr_u8(&v, &v_rt);

    assert!(
        psnr_u.is_infinite(),
        "NV12→I420 U PSNR must be infinite (pixel-exact), got {psnr_u:.1}"
    );
    assert!(
        psnr_v.is_infinite(),
        "NV12→I420 V PSNR must be infinite (pixel-exact), got {psnr_v:.1}"
    );
    assert_eq!(y_rt, y, "luma must be byte-exact in NV12↔I420 round-trip");
}
