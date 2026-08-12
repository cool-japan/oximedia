// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! End-to-end tests for the real frame-level transcode path
//! (`Pipeline::execute` with codec changes / filters).
//!
//! Every assertion verifies *content*, not just file existence: FLAC
//! output is re-decoded with the spec decoder and compared sample-exact,
//! MJPEG/MPEG-2 output is re-decoded with the real codec decoders, and
//! unsupported codecs must fail cleanly without creating output.

use std::path::PathBuf;

use oximedia_transcode::flac_decode::decode_flac_to_i16;
#[cfg(feature = "mjpeg")]
use oximedia_transcode::ScaleSpec;
use oximedia_transcode::TranscodePipelineBuilder;

// ─── Fixture helpers ──────────────────────────────────────────────────────────

/// Deterministic 16-bit PCM sine, interleaved.
fn sine_pcm(freq: f32, sr: u32, ch: u16, frames: usize, amp: f32) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * usize::from(ch));
    for i in 0..frames {
        let t = i as f32 / sr as f32;
        let v = ((2.0 * std::f32::consts::PI * freq * t).sin() * amp) as i16;
        for _ in 0..ch {
            out.push(v);
        }
    }
    out
}

/// Minimal valid 16-bit PCM WAV bytes.
fn wav_bytes(samples: &[i16], sr: u32, ch: u16) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let byte_rate = sr * u32::from(ch) * 2;
    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&ch.to_le_bytes());
    buf.extend_from_slice(&sr.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&(ch * 2).to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

/// Minimal Y4M (YUV4MPEG2 C420jpeg) with a moving gradient.
fn y4m_bytes(w: usize, h: usize, frames: usize, fps: (u32, u32)) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(
        format!(
            "YUV4MPEG2 W{w} H{h} F{}:{} Ip A1:1 C420jpeg\n",
            fps.0, fps.1
        )
        .as_bytes(),
    );
    for t in 0..frames {
        buf.extend_from_slice(b"FRAME\n");
        for y in 0..h {
            for x in 0..w {
                buf.push(((x * 4 + y * 2 + t * 8) % 256) as u8);
            }
        }
        for _ in 0..(h / 2) {
            for x in 0..(w / 2) {
                buf.push(((x * 8 + t * 4) % 256) as u8);
            }
        }
        for _ in 0..(h / 2) {
            for x in 0..(w / 2) {
                buf.push(((x * 8 + 256 - t * 4) % 256) as u8);
            }
        }
    }
    buf
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("oximedia_fl_e2e_{}_{name}", std::process::id()))
}

async fn run_pipeline(builder: TranscodePipelineBuilder) -> oximedia_transcode::Result<()> {
    let mut pipeline = builder.build()?;
    pipeline.execute().await.map(|_| ())
}

/// Extract the first JPEG image (SOI..EOI) embedded in a byte stream.
#[cfg(feature = "mjpeg")]
fn first_jpeg(data: &[u8]) -> Option<&[u8]> {
    let start = data.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])?;
    let end = data[start..].windows(2).position(|w| w == [0xFF, 0xD9])?;
    Some(&data[start..start + end + 2])
}

// ─── (a) Flagship: WAV → FLAC, sample-exact round trip ───────────────────────

#[tokio::test]
async fn wav_to_flac_round_trips_sample_exact() {
    let sr = 48_000u32;
    let src = sine_pcm(1_000.0, sr, 2, 48_000, 12_000.0);
    let input = temp_path("a_in.wav");
    let output = temp_path("a_out.flac");
    std::fs::write(&input, wav_bytes(&src, sr, 2)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .audio_codec("flac"),
    )
    .await
    .expect("wav→flac transcode must succeed");

    let flac = std::fs::read(&output).expect("read flac");
    assert!(flac.starts_with(b"fLaC"), "output must be a FLAC stream");
    assert!(
        flac.len() < src.len() * 2,
        "a sine must compress below raw PCM size ({} vs {})",
        flac.len(),
        src.len() * 2
    );

    let (params, decoded) = decode_flac_to_i16(&flac).expect("decode transcoded FLAC");
    assert_eq!(params.sample_rate, sr);
    assert_eq!(params.channels, 2);
    assert_eq!(
        params.total_samples, 48_000,
        "STREAMINFO total must be exact"
    );
    assert_eq!(decoded.len(), src.len(), "sample count must round-trip");
    assert_eq!(decoded, src, "16-bit FLAC transcode must be lossless");
    assert!(
        decoded.iter().any(|&s| s != 0),
        "decoded audio must not be silence"
    );

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── (b) WAV → FLAC with -af volume gain ─────────────────────────────────────

#[tokio::test]
async fn wav_to_flac_with_gain_shifts_level() {
    let sr = 48_000u32;
    let src = sine_pcm(500.0, sr, 1, 24_000, 12_000.0);
    let input = temp_path("b_in.wav");
    let output = temp_path("b_out.flac");
    std::fs::write(&input, wav_bytes(&src, sr, 1)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .audio_codec("flac")
            .audio_gain_db(-6.020_6),
    )
    .await
    .expect("wav→flac with gain must succeed");

    let (_, decoded) =
        decode_flac_to_i16(&std::fs::read(&output).expect("read flac")).expect("decode");
    let src_peak = src.iter().map(|s| i32::from(*s).abs()).max().unwrap_or(0);
    let out_peak = decoded
        .iter()
        .map(|s| i32::from(*s).abs())
        .max()
        .unwrap_or(0);
    let expected = src_peak / 2;
    assert!(
        (out_peak - expected).abs() < 100,
        "-6.02 dB must halve the peak: got {out_peak}, expected ≈{expected}"
    );

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── (c) Y4M → MJPEG in MKV, first frame re-decodes ──────────────────────────

#[cfg(feature = "mjpeg")]
#[tokio::test]
async fn y4m_to_mjpeg_mkv_first_frame_decodes() {
    use oximedia_codec::traits::VideoDecoder as _;

    let (w, h, n) = (64usize, 48usize, 8usize);
    let input = temp_path("c_in.y4m");
    let output = temp_path("c_out.mkv");
    std::fs::write(&input, y4m_bytes(w, h, n, (25, 1))).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .video_codec("mjpeg"),
    )
    .await
    .expect("y4m→mjpeg mkv transcode must succeed");

    let mkv = std::fs::read(&output).expect("read mkv");
    assert!(
        mkv.windows(7).any(|win| win == b"V_MJPEG"),
        "Matroska track must be labelled V_MJPEG"
    );
    let soi_count = mkv
        .windows(3)
        .filter(|win| win == &[0xFF, 0xD8, 0xFF])
        .count();
    assert_eq!(soi_count, n, "one JPEG image per source frame");

    // Re-decode the first embedded JPEG with the real MJPEG decoder.
    let jpeg = first_jpeg(&mkv).expect("an embedded JPEG image");
    let mut dec = oximedia_codec::MjpegDecoder::new(0, 0);
    dec.send_packet(jpeg, 0).expect("send jpeg");
    let frame = dec
        .receive_frame()
        .expect("decode call")
        .expect("one frame");
    assert_eq!(frame.width as usize, w, "decoded width");
    assert_eq!(frame.height as usize, h, "decoded height");
    // Plausible pixels: the gradient source is not flat.
    let plane = &frame.planes[0].data;
    let min = plane.iter().copied().min().unwrap_or(0);
    let max = plane.iter().copied().max().unwrap_or(0);
    assert!(
        max - min > 32,
        "decoded frame must show the source gradient (min {min}, max {max})"
    );

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── (d) -vf scale changes output dimensions ─────────────────────────────────

#[cfg(feature = "mjpeg")]
#[tokio::test]
async fn y4m_to_mjpeg_with_scale_changes_dims() {
    use oximedia_codec::traits::VideoDecoder as _;

    let input = temp_path("d_in.y4m");
    let output = temp_path("d_out.mkv");
    std::fs::write(&input, y4m_bytes(64, 48, 4, (25, 1))).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .video_codec("mjpeg")
            .video_scale(ScaleSpec {
                width: Some(32),
                height: Some(24),
            }),
    )
    .await
    .expect("scaled transcode must succeed");

    let mkv = std::fs::read(&output).expect("read mkv");
    let jpeg = first_jpeg(&mkv).expect("an embedded JPEG image");
    let mut dec = oximedia_codec::MjpegDecoder::new(0, 0);
    dec.send_packet(jpeg, 0).expect("send jpeg");
    let frame = dec
        .receive_frame()
        .expect("decode call")
        .expect("one frame");
    assert_eq!((frame.width, frame.height), (32, 24), "scaled dimensions");

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── Aspect-preserving scale (`-vf scale=32:-1`) ─────────────────────────────

#[cfg(feature = "mjpeg")]
#[tokio::test]
async fn scale_with_free_axis_preserves_aspect() {
    use oximedia_codec::traits::VideoDecoder as _;

    let input = temp_path("d2_in.y4m");
    let output = temp_path("d2_out.mkv");
    std::fs::write(&input, y4m_bytes(64, 48, 2, (25, 1))).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .video_codec("mjpeg")
            .video_scale(ScaleSpec {
                width: Some(32),
                height: None,
            }),
    )
    .await
    .expect("aspect-preserving scale must succeed");

    let mkv = std::fs::read(&output).expect("read mkv");
    let jpeg = first_jpeg(&mkv).expect("jpeg");
    let mut dec = oximedia_codec::MjpegDecoder::new(0, 0);
    dec.send_packet(jpeg, 0).expect("send");
    let frame = dec.receive_frame().expect("decode").expect("frame");
    assert_eq!(
        (frame.width, frame.height),
        (32, 24),
        "64x48 → 32x-1 = 32x24"
    );

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── -r frame-rate conversion at the pump level ──────────────────────────────

#[tokio::test]
async fn fps_conversion_duplicates_frames_for_upconversion() {
    let input = temp_path("r_in.y4m");
    let output = temp_path("r_out.y4m");
    std::fs::write(&input, y4m_bytes(32, 32, 10, (25, 1))).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .video_codec("rawvideo")
            .output_fps(50, 1),
    )
    .await
    .expect("25→50 fps rawvideo transcode must succeed");

    let out = std::fs::read(&output).expect("read y4m");
    let header_end = out
        .iter()
        .position(|&b| b == b'\n')
        .expect("y4m header line");
    let header = String::from_utf8_lossy(&out[..header_end]);
    assert!(
        header.contains("F50:1"),
        "output header must carry 50 fps: {header}"
    );
    let frames = out.windows(5).filter(|w| w == b"FRAME").count();
    assert_eq!(frames, 20, "25→50 fps must duplicate 10 frames to 20");

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── MPEG-2 elementary stream re-decodes ─────────────────────────────────────

#[cfg(feature = "mpeg2")]
#[tokio::test]
async fn y4m_to_mpeg2_es_decodes() {
    let input = temp_path("m2v_in.y4m");
    let output = temp_path("m2v_out.m2v");
    std::fs::write(&input, y4m_bytes(64, 48, 4, (25, 1))).expect("write y4m");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .video_codec("mpeg2"),
    )
    .await
    .expect("y4m→mpeg2 transcode must succeed");

    let es = std::fs::read(&output).expect("read m2v");
    // MPEG-2 sequence header start code.
    assert_eq!(&es[..4], &[0x00, 0x00, 0x01, 0xB3], "sequence header start");

    let dec = oximedia_codec::mpeg2::Mpeg2Decoder::new();
    let frame = dec.decode(&es).expect("decode first I-frame");
    assert_eq!(frame.width, 64);
    assert_eq!(frame.height, 48);
    let min = frame.y.iter().copied().min().unwrap_or(0);
    let max = frame.y.iter().copied().max().unwrap_or(0);
    assert!(max > min, "decoded luma must not be flat");

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── ALAC CAF output structure ───────────────────────────────────────────────

#[tokio::test]
async fn wav_to_alac_caf_structure_and_totals() {
    let sr = 44_100u32;
    let frames = 10_000usize;
    let src = sine_pcm(880.0, sr, 2, frames, 9_000.0);
    let input = temp_path("alac_in.wav");
    let output = temp_path("alac_out.caf");
    std::fs::write(&input, wav_bytes(&src, sr, 2)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .audio_codec("alac"),
    )
    .await
    .expect("wav→alac caf transcode must succeed");

    let caf = std::fs::read(&output).expect("read caf");
    assert_eq!(&caf[..4], b"caff");
    assert!(caf.windows(4).any(|w| w == b"desc"));
    assert!(caf.windows(4).any(|w| w == b"kuki"));
    assert!(caf.windows(4).any(|w| w == b"pakt"));
    assert!(caf.windows(4).any(|w| w == b"data"));

    // pakt header: number of packets and exact valid frames.
    let pakt_pos = caf
        .windows(4)
        .position(|w| w == b"pakt")
        .expect("pakt chunk");
    let body = &caf[pakt_pos + 12..];
    let num_packets = i64::from_be_bytes(body[0..8].try_into().expect("8 bytes"));
    let valid_frames = i64::from_be_bytes(body[8..16].try_into().expect("8 bytes"));
    assert_eq!(num_packets, 3, "10000 frames at 4096/packet = 3 packets");
    assert_eq!(valid_frames as usize, frames, "pakt total must be exact");

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── (e) Unsupported codecs fail cleanly ─────────────────────────────────────

#[tokio::test]
async fn unsupported_codecs_error_cleanly_without_output() {
    let src = sine_pcm(440.0, 48_000, 2, 4_800, 8_000.0);
    let wav_in = temp_path("e_in.wav");
    std::fs::write(&wav_in, wav_bytes(&src, 48_000, 2)).expect("write wav");
    let y4m_in = temp_path("e_in.y4m");
    std::fs::write(&y4m_in, y4m_bytes(32, 32, 2, (25, 1))).expect("write y4m");

    for (input, codec, is_video, needle) in [
        (&y4m_in, "vp9", true, "VP9"),
        (&y4m_in, "av1", true, "AV1"),
        (&y4m_in, "vp8", true, "VP8"),
        (&wav_in, "vorbis", false, "Vorbis"),
        (&wav_in, "opus", false, "Opus"),
        (&wav_in, "aac", false, "AAC"),
    ] {
        let output = temp_path(&format!("e_out_{codec}.mkv"));
        let _ = std::fs::remove_file(&output);
        let mut builder = TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone());
        builder = if is_video {
            builder.video_codec(codec)
        } else {
            builder.audio_codec(codec)
        };
        let err = run_pipeline(builder)
            .await
            .expect_err("unsupported codec must fail");
        let msg = err.to_string();
        assert!(
            msg.contains(needle),
            "error for {codec} must name the codec: {msg}"
        );
        assert!(
            !msg.contains("MultiTrackExecutor") && !msg.contains("FrameDecoder"),
            "error must not leak internals: {msg}"
        );
        assert!(
            !output.exists(),
            "no output file may be fabricated for {codec}"
        );
    }

    std::fs::remove_file(&wav_in).ok();
    std::fs::remove_file(&y4m_in).ok();
}

// ─── -ss / -t trim on the frame-level path ───────────────────────────────────

#[tokio::test]
async fn trim_reduces_flac_output_duration() {
    let sr = 48_000u32;
    let src = sine_pcm(1_000.0, sr, 2, 48_000, 10_000.0);
    let input = temp_path("trim_in.wav");
    let output = temp_path("trim_out.flac");
    std::fs::write(&input, wav_bytes(&src, sr, 2)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone())
            .audio_codec("flac")
            .start_time_secs(0.25)
            .duration_secs(0.5),
    )
    .await
    .expect("trimmed transcode must succeed");

    let (params, decoded) =
        decode_flac_to_i16(&std::fs::read(&output).expect("read")).expect("decode");
    let seconds = decoded.len() as f64 / 2.0 / f64::from(params.sample_rate);
    // Trim granularity is one 4096-sample chunk (≈85 ms at 48 kHz).
    assert!(
        (seconds - 0.5).abs() < 0.1,
        "-ss 0.25 -t 0.5 must keep ≈0.5 s, got {seconds:.3}"
    );

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}

// ─── Stream copy is untouched by the new gate ────────────────────────────────

#[tokio::test]
async fn pure_stream_copy_still_works() {
    // A WAV → MKA copy without codec overrides must stay on the
    // packet-level remux path and succeed as before.
    let src = sine_pcm(440.0, 48_000, 2, 9_600, 8_000.0);
    let input = temp_path("copy_in.wav");
    let output = temp_path("copy_out.mka");
    std::fs::write(&input, wav_bytes(&src, 48_000, 2)).expect("write wav");
    let _ = std::fs::remove_file(&output);

    run_pipeline(
        TranscodePipelineBuilder::new()
            .input(input.clone())
            .output(output.clone()),
    )
    .await
    .expect("stream copy must still succeed");

    let out = std::fs::read(&output).expect("read mka");
    assert!(!out.is_empty(), "copy output must not be empty");

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}
