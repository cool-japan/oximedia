// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Codec encoder factory for the intra-frame video codecs with real
//! implementations in `oximedia-codec`.
//!
//! This module provides [`make_video_encoder`], which constructs a boxed
//! [`oximedia_codec::VideoEncoder`] for a given [`CodecId`].  Dispatched
//! codecs:
//!
//! | `CodecId`         | Encoder              | Feature gate | Input           |
//! |-------------------|----------------------|--------------|-----------------|
//! | `CodecId::Mjpeg`  | `MjpegEncoder`       | `mjpeg`      | 8-bit YUV/RGB   |
//! | `CodecId::Apv`    | `ApvEncoder`         | `apv`        | 8-bit YUV/RGB   |
//! | `CodecId::Mpeg2`  | `Mpeg2Encoder`       | `mpeg2`      | 8-bit YUV 4:2:0 |
//! | `CodecId::Ffv1`   | `Ffv1Encoder`        | `ffv1`       | 8-bit YUV 4:2:0 |
//! | `CodecId::ProRes` | `ProResEncoder`      | `prores`     | 10-bit 4:2:2 LE |
//!
//! `quality` interpretation is codec-specific — see [`VideoEncoderParams`].
//! AV1/VP9/VP8 are intentionally not dispatched: their encode paths do not
//! produce real output yet, and callers get a descriptive `Unsupported`
//! error instead of a fabricated file.

use crate::{Result, TranscodeError};
use oximedia_codec::traits::VideoEncoder;
#[cfg(feature = "mjpeg")]
use oximedia_codec::CodecError;
use oximedia_core::CodecId;

/// Parameters used to instantiate an intra-frame video encoder.
#[derive(Debug, Clone)]
pub struct VideoEncoderParams {
    /// Frame width in pixels (must be > 0).
    pub width: u32,
    /// Frame height in pixels (must be > 0).
    pub height: u32,
    /// Quality/QP value.  Interpretation depends on the codec:
    /// - MJPEG: JPEG quality 1-100 (higher = better).
    /// - APV: quantisation parameter 0-63 (lower = better).
    pub quality: u8,
}

impl VideoEncoderParams {
    /// Create a new parameter set.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] if width or height is zero.
    pub fn new(width: u32, height: u32, quality: u8) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(TranscodeError::InvalidInput(
                "width and height must be non-zero".into(),
            ));
        }
        Ok(Self {
            width,
            height,
            quality,
        })
    }
}

/// Build a boxed [`VideoEncoder`] for the specified codec.
///
/// # Errors
///
/// - [`TranscodeError::Unsupported`] if `codec_id` has no real encoder
///   (AV1/VP9/VP8) or its feature is not compiled in.
/// - [`TranscodeError::CodecError`] if the underlying encoder rejects the
///   parameters.
pub fn make_video_encoder(
    codec_id: CodecId,
    params: &VideoEncoderParams,
) -> Result<Box<dyn VideoEncoder>> {
    match codec_id {
        CodecId::Mjpeg => make_mjpeg_encoder(params),
        CodecId::Apv => make_apv_encoder(params),
        CodecId::Mpeg2 => make_mpeg2_encoder(params),
        CodecId::Ffv1 => make_ffv1_encoder(params),
        CodecId::ProRes => make_prores_encoder(params),
        other => Err(TranscodeError::Unsupported(format!(
            "codec {other:?} has no real encoder in this build; \
             transcoding to it is not yet supported"
        ))),
    }
}

// ─── MJPEG ───────────────────────────────────────────────────────────────────

#[cfg(feature = "mjpeg")]
fn make_mjpeg_encoder(params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    use oximedia_codec::{MjpegConfig, MjpegEncoder};

    let config = MjpegConfig::new(params.width, params.height)
        .map_err(|e| TranscodeError::CodecError(e.to_string()))?
        .with_quality(params.quality);

    let encoder = MjpegEncoder::new(config)
        .map_err(|e: CodecError| TranscodeError::CodecError(e.to_string()))?;

    Ok(Box::new(encoder))
}

#[cfg(not(feature = "mjpeg"))]
fn make_mjpeg_encoder(_params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    Err(TranscodeError::Unsupported(
        "MJPEG support requires the `mjpeg` feature of oximedia-codec".into(),
    ))
}

// ─── APV ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "apv")]
fn make_apv_encoder(params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    use oximedia_codec::{ApvConfig, ApvEncoder};

    let config = ApvConfig::new(params.width, params.height)
        .map_err(|e| TranscodeError::CodecError(e.to_string()))?
        .with_qp(params.quality);

    let encoder = ApvEncoder::new(config).map_err(|e| TranscodeError::CodecError(e.to_string()))?;

    Ok(Box::new(encoder))
}

#[cfg(not(feature = "apv"))]
fn make_apv_encoder(_params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    Err(TranscodeError::Unsupported(
        "APV support requires the `apv` feature of oximedia-codec".into(),
    ))
}

// ─── MPEG-2 ──────────────────────────────────────────────────────────────────

#[cfg(feature = "mpeg2")]
fn make_mpeg2_encoder(params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    use oximedia_codec::mpeg2::{Mpeg2Encoder, Mpeg2EncoderConfig};

    // MPEG-2 qscale range is 1..=31 (lower = better quality).
    let qscale = params.quality.clamp(1, 31);
    let config = Mpeg2EncoderConfig::yuv420p(params.width, params.height, qscale);
    let encoder =
        Mpeg2Encoder::new(config).map_err(|e| TranscodeError::CodecError(e.to_string()))?;
    Ok(Box::new(encoder))
}

#[cfg(not(feature = "mpeg2"))]
fn make_mpeg2_encoder(_params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    Err(TranscodeError::Unsupported(
        "MPEG-2 support requires the `mpeg2` feature of oximedia-codec".into(),
    ))
}

// ─── FFV1 ────────────────────────────────────────────────────────────────────

#[cfg(feature = "ffv1")]
fn ffv1_encoder_config(params: &VideoEncoderParams) -> oximedia_codec::traits::EncoderConfig {
    use oximedia_codec::traits::EncoderConfig;
    use oximedia_core::PixelFormat;

    // FFV1 is lossless — `quality` has no effect. All-intra (keyint = 1)
    // keeps every frame independently decodable.
    EncoderConfig {
        codec: CodecId::Ffv1,
        width: params.width,
        height: params.height,
        pixel_format: PixelFormat::Yuv420p,
        keyint: 1,
        ..EncoderConfig::default()
    }
}

#[cfg(feature = "ffv1")]
fn make_ffv1_encoder(params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    use oximedia_codec::Ffv1Encoder;

    let encoder = Ffv1Encoder::new(ffv1_encoder_config(params))
        .map_err(|e| TranscodeError::CodecError(e.to_string()))?;
    Ok(Box::new(encoder))
}

#[cfg(not(feature = "ffv1"))]
fn make_ffv1_encoder(_params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    Err(TranscodeError::Unsupported(
        "FFV1 support requires the `ffv1` feature of oximedia-codec".into(),
    ))
}

/// Computes the FFV1 extradata (configuration record) for `params` without
/// keeping the encoder around — used by the frame-level engine's raw-FFV1
/// sink, which needs the extradata *before* any packets are produced (to
/// write it into the file header) but consumes the actual encoder only
/// through the boxed [`VideoEncoder`] trait object from
/// [`make_video_encoder`]. Deterministic: `Ffv1Encoder::new` derives
/// `Ffv1Config` purely from `width`/`height`, so this always matches the
/// extradata the real encoder for the same `params` would report.
///
/// # Errors
///
/// Same as [`make_video_encoder`] for [`oximedia_core::CodecId::Ffv1`].
#[cfg(feature = "ffv1")]
pub fn ffv1_extradata(params: &VideoEncoderParams) -> Result<Vec<u8>> {
    use oximedia_codec::Ffv1Encoder;

    let encoder = Ffv1Encoder::new(ffv1_encoder_config(params))
        .map_err(|e| TranscodeError::CodecError(e.to_string()))?;
    Ok(encoder.extradata())
}

/// See the `ffv1` feature-enabled overload.
///
/// # Errors
///
/// Always returns [`TranscodeError::Unsupported`] — the `ffv1` feature of
/// `oximedia-codec` is not compiled in.
#[cfg(not(feature = "ffv1"))]
pub fn ffv1_extradata(_params: &VideoEncoderParams) -> Result<Vec<u8>> {
    Err(TranscodeError::Unsupported(
        "FFV1 support requires the `ffv1` feature of oximedia-codec".into(),
    ))
}

// ─── ProRes ──────────────────────────────────────────────────────────────────

#[cfg(feature = "prores")]
fn make_prores_encoder(params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    use oximedia_codec::{ProResEncoder, ProResEncoderConfig, ProResProfile};

    // The real ProRes encoder requires 16-pixel-aligned dimensions and
    // 10-bit 4:2:2 (`Yuv422p10le`) input frames.
    if params.width % 16 != 0 || params.height % 16 != 0 {
        return Err(TranscodeError::CodecError(format!(
            "ProRes requires dimensions that are multiples of 16, got {}x{}",
            params.width, params.height
        )));
    }
    let config = ProResEncoderConfig::new(ProResProfile::Standard, params.width, params.height);
    let encoder =
        ProResEncoder::new(config).map_err(|e| TranscodeError::CodecError(e.to_string()))?;
    Ok(Box::new(encoder))
}

#[cfg(not(feature = "prores"))]
fn make_prores_encoder(_params: &VideoEncoderParams) -> Result<Box<dyn VideoEncoder>> {
    Err(TranscodeError::Unsupported(
        "ProRes support requires the `prores` feature of oximedia-codec".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_new_valid() {
        let p = VideoEncoderParams::new(1920, 1080, 85);
        assert!(p.is_ok());
        let p = p.expect("valid params");
        assert_eq!(p.width, 1920);
        assert_eq!(p.height, 1080);
        assert_eq!(p.quality, 85);
    }

    #[test]
    fn test_params_zero_width() {
        assert!(VideoEncoderParams::new(0, 1080, 85).is_err());
    }

    #[test]
    fn test_params_zero_height() {
        assert!(VideoEncoderParams::new(1920, 0, 85).is_err());
    }

    #[test]
    fn test_unsupported_codec() {
        let p = VideoEncoderParams::new(320, 240, 30).expect("valid");
        let result = make_video_encoder(CodecId::Vp9, &p);
        assert!(result.is_err());
        // Extract the error without requiring Debug on the Ok variant.
        if let Err(e) = result {
            assert!(matches!(e, TranscodeError::Unsupported(_)));
        }
    }

    #[cfg(feature = "mjpeg")]
    #[test]
    fn test_make_mjpeg_encoder() {
        let p = VideoEncoderParams::new(320, 240, 85).expect("valid");
        let enc = make_video_encoder(CodecId::Mjpeg, &p);
        assert!(enc.is_ok(), "MJPEG encoder should build");
        let enc = enc.expect("ok");
        assert_eq!(enc.codec(), CodecId::Mjpeg);
    }

    #[cfg(feature = "apv")]
    #[test]
    fn test_make_apv_encoder() {
        let p = VideoEncoderParams::new(320, 240, 22).expect("valid");
        let enc = make_video_encoder(CodecId::Apv, &p);
        assert!(enc.is_ok(), "APV encoder should build");
        let enc = enc.expect("ok");
        assert_eq!(enc.codec(), CodecId::Apv);
    }

    #[cfg(not(feature = "mjpeg"))]
    #[test]
    fn test_mjpeg_disabled() {
        let p = VideoEncoderParams::new(320, 240, 85).expect("valid");
        let result = make_video_encoder(CodecId::Mjpeg, &p);
        assert!(matches!(result, Err(TranscodeError::Unsupported(_))));
    }

    #[cfg(not(feature = "apv"))]
    #[test]
    fn test_apv_disabled() {
        let p = VideoEncoderParams::new(320, 240, 22).expect("valid");
        let result = make_video_encoder(CodecId::Apv, &p);
        assert!(matches!(result, Err(TranscodeError::Unsupported(_))));
    }

    // ── ProRes: what does this codec genuinely support? ───────────────────
    //
    // This is the empirical basis for frame_level.rs's honest-Err message
    // on the ProRes video target (task: "check what the in-tree ProRes
    // codec genuinely supports"). Findings, verified here:
    //
    // - The encoder is real: it accepts genuine 10-bit 4:2:2 (`Yuv422p10le`)
    //   input and produces a real ProRes 'icpf' bitstream (verified below:
    //   distinct 10-bit inputs produce distinct, non-trivial output).
    // - The decoder is real but its output is capped at 8 bits: both
    //   `ProResDecoder::decode` and the `VideoDecoder::send_packet` /
    //   `receive_frame` path right-shift the internally-reconstructed
    //   10-bit planes by 2 before returning them (see
    //   `oximedia_codec::prores::decoder::decode_impl`'s "Convert 10-bit
    //   planes to 8-bit output" step) — there is no API that returns 10-bit
    //   decoded samples. So even with a genuine 10-bit source, this codec
    //   cannot deliver a 10-bit-in/10-bit-out round trip.
    /// Builds a plane split into 4 constant-value quadrants — genuinely
    /// varying (non-degenerate) 10-bit content that still aligns with
    /// 8×8-ish DCT block boundaries, so quantization error stays small
    /// (matching the flat-frame tolerances `oximedia-codec`'s own
    /// `prores_roundtrip.rs` suite uses; an arbitrary per-pixel pattern
    /// is adversarial for any DCT codec and isn't representative of real
    /// video).
    #[cfg(feature = "prores")]
    fn quadrant_plane(width: usize, height: usize, values: [u16; 4]) -> Vec<u16> {
        let (half_w, half_h) = (width / 2, height / 2);
        let mut out = Vec::with_capacity(width * height);
        for row in 0..height {
            for col in 0..width {
                let qi = usize::from(col >= half_w) + 2 * usize::from(row >= half_h);
                out.push(values[qi]);
            }
        }
        out
    }

    #[cfg(feature = "prores")]
    #[test]
    fn test_prores_genuine_10bit_encode_8bit_decode_roundtrip() {
        use oximedia_codec::frame::{Plane, VideoFrame};
        use oximedia_codec::prores::{ProResDecoder, ProResEncoderConfig, ProResProfile};
        use oximedia_codec::traits::VideoEncoder;
        use oximedia_codec::ProResEncoder;
        use oximedia_core::PixelFormat;

        // 16-pixel-aligned (ProRes slice requirement), genuinely 10-bit,
        // genuinely varying (4 distinct quadrant values, moderate contrast)
        // — NOT derived by shifting 8-bit data, and not so high-contrast
        // that it hits the entropy-decoder limitation documented in
        // `test_prores_decoder_rejects_high_contrast_content` below.
        let (w, h) = (16u32, 16u32);
        let cw = (w / 2) as usize;
        let y_10bit = quadrant_plane(w as usize, h as usize, [420, 480, 540, 600]);
        let cb_10bit = quadrant_plane(cw, h as usize, [460, 520, 500, 560]);
        let cr_10bit = quadrant_plane(cw, h as usize, [560, 500, 520, 460]);

        let to_le_bytes =
            |samples: &[u16]| -> Vec<u8> { samples.iter().flat_map(|s| s.to_le_bytes()).collect() };

        let mut frame = VideoFrame::new(PixelFormat::Yuv422p10le, w, h);
        frame.planes = vec![
            Plane::with_dimensions(to_le_bytes(&y_10bit), w as usize * 2, w, h),
            Plane::with_dimensions(to_le_bytes(&cb_10bit), cw * 2, w / 2, h),
            Plane::with_dimensions(to_le_bytes(&cr_10bit), cw * 2, w / 2, h),
        ];

        let config = ProResEncoderConfig::new(ProResProfile::Standard, w, h);
        let mut encoder = ProResEncoder::new(config).expect("real 10-bit prores encoder");
        encoder
            .send_frame(&frame)
            .expect("encode genuine 10-bit frame");
        let packet = encoder
            .receive_packet()
            .expect("receive_packet")
            .expect("encoder must produce a packet for one sent frame");
        assert!(
            packet.data.len() > 16,
            "encoded ProRes packet suspiciously small: {} bytes",
            packet.data.len()
        );

        let decoded = ProResDecoder::decode(&packet.data).expect("real prores decode");
        assert_eq!(decoded.width, w);
        assert_eq!(decoded.height, h);
        assert_eq!(
            decoded.y.len(),
            y_10bit.len(),
            "decoder output is 8-bit (not 10-bit)"
        );

        // The decoder's contract is "10-bit internal reconstruction >> 2".
        // ProRes is a DCT/quantization codec (visually, not mathematically,
        // lossless), so compare against that expectation within tolerance,
        // not bit-exact.
        let expected_y8: Vec<u8> = y_10bit.iter().map(|&s| (s >> 2) as u8).collect();
        let max_diff = expected_y8
            .iter()
            .zip(decoded.y.iter())
            .map(|(&a, &b)| (i32::from(a) - i32::from(b)).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 8,
            "10-bit-encode -> 8-bit-decode luma drifted too far: max diff {max_diff} \
             (encoder/decoder pipeline is not functioning as real DCT codec)"
        );

        // A different 10-bit source must produce different encoded bytes —
        // guards against a stub encoder that ignores its input.
        let mut frame2 = frame.clone();
        frame2.planes[0].data =
            to_le_bytes(&y_10bit.iter().map(|&s| 1023 - s).collect::<Vec<u16>>());
        let mut encoder2 =
            ProResEncoder::new(ProResEncoderConfig::new(ProResProfile::Standard, w, h))
                .expect("encoder 2");
        encoder2.send_frame(&frame2).expect("encode inverted frame");
        let packet2 = encoder2
            .receive_packet()
            .expect("receive_packet 2")
            .expect("packet 2");
        assert_ne!(
            packet.data, packet2.data,
            "encoder must not ignore its 10-bit input"
        );
    }

    /// Documents a further finding beyond the 8-bit-decode ceiling: this
    /// build's `ProResDecoder` also fails outright — `DecoderError`,
    /// "entropy decode: malformed codeword (unary prefix too long)" — on
    /// content with a large sample-value swing within one macroblock (a
    /// 500 → 900 step reproduces it; 500 → 600 does not), even though
    /// `ProResEncoder::send_frame`/`receive_packet` succeed and produce a
    /// non-trivial packet for the same input. High-contrast content (a
    /// sharp edge, a title card, a specular highlight) is unremarkable in
    /// real video, so this is a real robustness gap in the encoder/decoder
    /// pair, not just a synthetic-test artifact — a second, independent
    /// reason (beyond the pipeline/bit-depth issues) that ProRes stays
    /// honest-`Err` in `frame_level.rs` rather than being wired up.
    ///
    /// This asserts today's (broken) behavior on purpose: if a future
    /// `oximedia-codec` fix makes this decode succeed, this test starts
    /// failing, which is the signal to revisit the ProRes wiring decision
    /// — not a bug in this test.
    #[cfg(feature = "prores")]
    #[test]
    fn test_prores_decoder_rejects_high_contrast_content() {
        use oximedia_codec::frame::{Plane, VideoFrame};
        use oximedia_codec::prores::{ProResDecoder, ProResEncoderConfig, ProResProfile};
        use oximedia_codec::traits::VideoEncoder;
        use oximedia_codec::ProResEncoder;
        use oximedia_core::PixelFormat;

        let (w, h) = (16u32, 16u32);
        let cw = (w / 2) as usize;
        // A hard 500 -> 900 edge at the halfway column, one macroblock.
        let y_10bit: Vec<u16> = (0..h)
            .flat_map(|_row| (0..w).map(|col| if col < 8 { 500u16 } else { 900u16 }))
            .collect();
        let flat_chroma = vec![512u16; cw * h as usize];
        let to_le_bytes =
            |samples: &[u16]| -> Vec<u8> { samples.iter().flat_map(|s| s.to_le_bytes()).collect() };
        let mut frame = VideoFrame::new(PixelFormat::Yuv422p10le, w, h);
        frame.planes = vec![
            Plane::with_dimensions(to_le_bytes(&y_10bit), w as usize * 2, w, h),
            Plane::with_dimensions(to_le_bytes(&flat_chroma), cw * 2, w / 2, h),
            Plane::with_dimensions(to_le_bytes(&flat_chroma), cw * 2, w / 2, h),
        ];
        let config = ProResEncoderConfig::new(ProResProfile::Standard, w, h);
        let mut encoder = ProResEncoder::new(config).expect("encoder accepts the frame");
        encoder
            .send_frame(&frame)
            .expect("encoder accepts high-contrast content");
        let packet = encoder
            .receive_packet()
            .expect("receive_packet")
            .expect("encoder produces a packet");

        let result = ProResDecoder::decode(&packet.data);
        assert!(
            result.is_err(),
            "expected the known entropy-decoder limitation on high-contrast \
             content to still reproduce; if this now succeeds, ProRes may be \
             ready to reconsider for frame_level.rs wiring"
        );
    }
}
