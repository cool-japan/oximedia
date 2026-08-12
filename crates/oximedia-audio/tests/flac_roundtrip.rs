//! FLAC encode -> decode round-trip conformance tests.
//!
//! `conformance_tests.rs` already has tests named "FLAC round-trip", but none
//! of them actually decode the encoder's output — they only check packet
//! shape (sync bytes, packet count, size heuristics). Every test in *this*
//! file encodes real PCM, decodes the encoder's own output back through
//! `FlacDecoder`, and asserts the recovered samples are bit-exact with the
//! source. That is the actual definition of "lossless", and it is what
//! caught (and now guards against regressing) the encoder/decoder bugs fixed
//! alongside this file:
//!
//! 1. **Unary/Rice-code bit polarity was inverted.** `BitWriter::write_unary`
//!    and `BitReader::read_unary` used "N ones then a terminating zero", but
//!    RFC 9639 section 9.2.7 (the FLAC spec) defines FLAC's unary coding as
//!    "N zeros then a terminating one" — the opposite polarity. Both the
//!    encoder and the decoder had the same inversion, so they agreed with
//!    each other (and with `crate::flac::rice`'s otherwise-unused `BitReader`,
//!    which had the identical bug) — every round trip through *this*
//!    codebase's own decoder passed, byte-for-byte CRC included, while every
//!    file this encoder ever produced was unreadable by any spec-compliant
//!    decoder (confirmed independently with the system `ffmpeg`, whose
//!    `flacdec.c` implements RFC 9639's polarity and rejected the
//!    pre-fix output with "invalid residual"). This is why
//!    `flac_ffmpeg_cross_check` below exists and is the decisive test: no
//!    amount of self-consistent round-tripping through this crate's own
//!    encoder/decoder pair can distinguish "correct" from "consistently
//!    backwards".
//! 2. `encode_fixed_subframe` / `encode_lpc_subframe` built the subframe
//!    header byte by OR-ing the predictor order directly into the low bits
//!    instead of shifting the 6-bit type field left by one first. This wrote
//!    the wrong subframe type for almost every order and spuriously set the
//!    wasted-bits flag for others, desyncing the decoder's bit position for
//!    the rest of the frame.
//! 3. `encode_residuals` partitioned the residual array as
//!    `residuals.len() / partition_count` (remainder in the last partition),
//!    while the decoder (correctly) expects the first partition to be
//!    `predictor_order` samples short and every other partition to hold
//!    exactly `block_size >> partition_order`. These only agree when
//!    `partition_order == 0`, i.e. never at the default compression level.
//! 4. `encode_lpc_subframe` wrote coefficients truncated to `precision` bits
//!    but computed residuals from the untruncated values. This does not
//!    desync the bitstream (so CRC-16 still passes) — it silently recovers
//!    the wrong PCM. Low-frequency content is exactly what drives an LPC
//!    coefficient up against the truncation boundary, hence the frequency
//!    sweep in `flac_round_trip_sine_sweep_mono` below.
//! 5. The decoder never verified the frame header's CRC-8 at all (parsed and
//!    stored, never compared) — only CRC-16 over the whole frame was checked.
//!
//! Bugs 2-4 meant every *pre-existing* test (constant signals, or
//! non-constant signals only ever encoded at `CompressionLevel::FASTEST`,
//! which tries neither FIXED order >= 1 nor LPC) passed while the encoder was
//! broken for any real signal at the default compression level — that much
//! was catchable from inside this crate. Bug 1 could not be: it required an
//! independent, spec-compliant decoder to surface at all.

use bytes::Bytes;
use oximedia_audio::{
    flac::{CompressionLevel, FlacDecoder, FlacEncoder},
    frame::{AudioBuffer, AudioFrame},
    AudioDecoder, AudioDecoderConfig, AudioEncoder, AudioEncoderConfig, ChannelLayout,
};
use oximedia_core::{CodecId, Rational, SampleFormat, Timestamp};

// ---------------------------------------------------------------------------
// Signal generators
// ---------------------------------------------------------------------------

/// Deterministic xorshift32 PRNG, so noise fixtures don't need a `rand` dev-dependency.
struct Xorshift32(u32);

impl Xorshift32 {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0xDEAD_BEEF } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// Pure sine wave, quantized to `i16`.
fn sine_i16(freq_hz: f64, amplitude: f64, sample_rate: u32, num_samples: usize) -> Vec<i16> {
    let sr = f64::from(sample_rate);
    (0..num_samples)
        .map(|n| {
            let t = n as f64 / sr;
            let s = amplitude * (2.0 * std::f64::consts::PI * freq_hz * t).sin();
            (s * f64::from(i16::MAX)) as i16
        })
        .collect()
}

/// White noise at roughly `amplitude` (0..1) of full scale.
fn noise_i16(seed: u32, amplitude: f64, num_samples: usize) -> Vec<i16> {
    let mut rng = Xorshift32::new(seed);
    (0..num_samples)
        .map(|_| {
            // Fold to i16 range, then scale down to `amplitude` of full scale.
            let raw = (rng.next_u32() >> 16) as i16;
            (f64::from(raw) * amplitude) as i16
        })
        .collect()
}

fn silence_i16(num_samples: usize) -> Vec<i16> {
    vec![0i16; num_samples]
}

// ---------------------------------------------------------------------------
// Encode / decode helpers
// ---------------------------------------------------------------------------

/// Encode `samples_per_channel` (one `Vec<i16>` per channel, all equal length)
/// into a complete FLAC stream: `"fLaC"` marker + STREAMINFO metadata block +
/// encoded frames — exactly the bytes of a `.flac` file on disk.
fn encode_flac_stream(
    samples_per_channel: &[Vec<i16>],
    sample_rate: u32,
    block_size: u32,
    level: CompressionLevel,
) -> Vec<u8> {
    let channels = samples_per_channel.len() as u8;
    let total_samples = samples_per_channel[0].len() as u64;

    let config = AudioEncoderConfig {
        codec: CodecId::Flac,
        sample_rate,
        channels,
        bitrate: 0,
        frame_size: block_size,
    };
    let mut enc = FlacEncoder::with_compression_level(&config, level).expect("encoder creation");

    let mut stream = Vec::new();
    stream.extend_from_slice(b"fLaC");

    let si_data = enc
        .generate_streaminfo(total_samples)
        .expect("streaminfo generation");
    stream.push(0x80); // last metadata block, type 0 (STREAMINFO)
    stream.push(0x00);
    stream.push(0x00);
    stream.push(0x22); // 34 bytes
    stream.extend_from_slice(&si_data);

    let sample_count = samples_per_channel[0].len();
    let mut start = 0usize;
    while start < sample_count {
        let end = (start + block_size as usize).min(sample_count);

        let mut interleaved = Vec::with_capacity((end - start) * channels as usize * 2);
        for s in start..end {
            for ch in samples_per_channel {
                interleaved.extend_from_slice(&ch[s].to_le_bytes());
            }
        }

        let frame = AudioFrame {
            format: SampleFormat::S16,
            sample_rate,
            channels: ChannelLayout::from_count(channels as usize),
            samples: AudioBuffer::Interleaved(Bytes::from(interleaved)),
            timestamp: Timestamp::new(start as i64, Rational::new(1, i64::from(sample_rate))),
        };
        enc.send_frame(&frame).expect("send_frame");
        while let Some(pkt) = enc.receive_packet().expect("receive_packet") {
            stream.extend_from_slice(&pkt.data);
        }
        start = end;
    }

    enc.flush().expect("flush");
    while let Some(pkt) = enc.receive_packet().expect("receive_packet after flush") {
        stream.extend_from_slice(&pkt.data);
    }

    stream
}

/// Decode a complete FLAC stream, returning one `Vec<i32>` of recovered
/// samples per channel. The decoder emits `f32` samples as `s as f32 /
/// 32768.0`; since division/multiplication by a power of two is exact in
/// IEEE-754, `(f * 32768.0).round() as i32` recovers the original integer
/// exactly (not approximately) as long as the round trip really is lossless,
/// so callers can assert `==` rather than an epsilon tolerance.
fn decode_flac_stream(data: &[u8], channels: usize, sample_rate: u32) -> Vec<Vec<i32>> {
    let config = AudioDecoderConfig {
        codec: CodecId::Flac,
        sample_rate,
        channels: channels as u8,
        extradata: None,
    };
    let mut dec = FlacDecoder::new(&config).expect("decoder creation");
    dec.send_packet(data, 0).expect("send_packet");

    let max_val = f64::from(1i32 << 15) as f32; // encoder is fixed at 16 bits/sample
    let mut result: Vec<Vec<i32>> = vec![Vec::new(); channels];

    while let Some(frame) = dec.receive_frame().expect("receive_frame (CRC rejected?)") {
        if let AudioBuffer::Planar(planes) = &frame.samples {
            for (ch, plane) in planes.iter().enumerate() {
                if ch < channels {
                    for chunk in plane.chunks_exact(4) {
                        let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        result[ch].push((f * max_val).round() as i32);
                    }
                }
            }
        }
    }

    result
}

/// Encode, decode, and assert the recovered samples are bit-exact with the
/// source on every channel. This is the round trip: no CRC-8/CRC-16
/// rejection (an `Err` from `decode_flac_stream` panics via `.expect`
/// above) and exact sample recovery.
fn assert_round_trip_exact(
    samples_per_channel: &[Vec<i16>],
    sample_rate: u32,
    block_size: u32,
    level: CompressionLevel,
    label: &str,
) {
    let stream = encode_flac_stream(samples_per_channel, sample_rate, block_size, level);
    let decoded = decode_flac_stream(&stream, samples_per_channel.len(), sample_rate);

    for (ch, expected) in samples_per_channel.iter().enumerate() {
        let got = &decoded[ch];
        assert_eq!(
            got.len(),
            expected.len(),
            "{label}: channel {ch} sample count mismatch (decoded {} vs source {})",
            got.len(),
            expected.len()
        );
        for (i, (&e, &g)) in expected.iter().zip(got.iter()).enumerate() {
            assert_eq!(
                i32::from(e),
                g,
                "{label}: channel {ch} sample {i} not bit-exact (source {e}, decoded {g})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Round-trip tests: single-channel content
// ---------------------------------------------------------------------------

#[test]
fn flac_round_trip_silence_mono() {
    let samples = silence_i16(4096 * 3);
    assert_round_trip_exact(
        &[samples],
        44100,
        4096,
        CompressionLevel::DEFAULT,
        "silence",
    );
}

/// Sweeps several frequencies, including a very low one. A 2nd-order fixed
/// or LPC predictor for a slow sinusoid has `a1 ~= 2*cos(2*pi*f/sr)`, which
/// approaches 2.0 as `f -> 0` — right at (and, once quantized, sometimes
/// past) the 12-bit signed coefficient range `[-2048, 2047]`. This is the
/// scenario that exercises the coefficient-clamp fix in `encode_lpc_subframe`.
#[test]
fn flac_round_trip_sine_sweep_mono() {
    let sample_rate = 44100u32;
    let block_size = 2048u32;
    for freq in [5.0, 20.0, 100.0, 440.0, 1000.0, 5000.0, 11025.0, 16000.0] {
        let samples = sine_i16(freq, 0.85, sample_rate, block_size as usize * 4);
        assert_round_trip_exact(
            &[samples],
            sample_rate,
            block_size,
            CompressionLevel::DEFAULT,
            &format!("sine {freq}Hz"),
        );
    }
}

#[test]
fn flac_round_trip_white_noise_mono() {
    let sample_rate = 16000u32;
    let samples = noise_i16(12345, 0.7, 4096 * 3);
    assert_round_trip_exact(
        &[samples],
        sample_rate,
        4096,
        CompressionLevel::DEFAULT,
        "white noise",
    );
}

// ---------------------------------------------------------------------------
// Round-trip tests: multi-channel content
// ---------------------------------------------------------------------------

/// Uncorrelated channels: the encoder should keep `ChannelAssignment::Independent`.
#[test]
fn flac_round_trip_stereo_independent() {
    let sample_rate = 44100u32;
    let block_size = 1024u32;
    let n = block_size as usize * 3;
    let left = sine_i16(440.0, 0.6, sample_rate, n);
    let right = noise_i16(999, 0.5, n);
    assert_round_trip_exact(
        &[left, right],
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
        "stereo independent",
    );
}

/// Highly correlated channels: the encoder should prefer LeftSide/RightSide/
/// MidSide decorrelation, exercising `apply_decorrelation` on the decode side.
#[test]
fn flac_round_trip_stereo_correlated_midside() {
    let sample_rate = 44100u32;
    let block_size = 1024u32;
    let n = block_size as usize * 3;
    let left = sine_i16(220.0, 0.7, sample_rate, n);
    // Right is left with a small constant offset and slight scale — highly
    // correlated but not identical, so `side = left - right` is small but
    // non-zero for every sample.
    let right: Vec<i16> = left
        .iter()
        .map(|&l| {
            (i32::from(l) * 97 / 100 + 37).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
        })
        .collect();
    assert_round_trip_exact(
        &[left, right],
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
        "stereo mid/side",
    );
}

/// FLAC supports 1-8 independent channels; only 2-channel input gets stereo
/// decorrelation. Exercise a channel count that always takes the plain
/// `ChannelAssignment::Independent(n)` path.
#[test]
fn flac_round_trip_four_channel_independent() {
    let sample_rate = 48000u32;
    let block_size = 512u32;
    let n = block_size as usize * 2;
    let channels: Vec<Vec<i16>> = (0..4u32)
        .map(|i| sine_i16(110.0 * f64::from(i + 1), 0.5, sample_rate, n))
        .collect();
    assert_round_trip_exact(
        &channels,
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
        "four channel independent",
    );
}

// ---------------------------------------------------------------------------
// Round-trip tests: block sizes crossing frame boundaries
// ---------------------------------------------------------------------------

/// Total sample counts that are *not* an exact multiple of `frame_size`,
/// producing a ragged final block. This exercises:
/// - the partition-order clamp in `encode_residuals` (the final block's size
///   may not be divisible by `2^partition_order`), and
/// - the `order > samples.len()` guard in `encode_fixed_subframe` /
///   the pre-existing equivalent check inside `calculate_lpc_coefficients`
///   for `encode_lpc_subframe` (the final block may be shorter than the
///   predictor order the compression level would otherwise try).
#[test]
fn flac_round_trip_ragged_final_block() {
    let sample_rate = 44100u32;
    let block_size = 1000u32;

    // (total_samples, label) — leftover = total_samples % block_size:
    //   5    -> smaller than every fixed/LPC order tried at DEFAULT
    //   502  -> divisible by 2 but not 4: exercises one step of the
    //           partition-order backoff (2 -> 1)
    //   333  -> not divisible by 2 or 4: backs all the way off to order 0
    for total in [3005usize, 3502, 3333] {
        let samples = sine_i16(300.0, 0.6, sample_rate, total);
        assert_round_trip_exact(
            &[samples],
            sample_rate,
            block_size,
            CompressionLevel::DEFAULT,
            &format!("ragged final block (total={total})"),
        );
    }
}

/// `write_utf8_u32`/frame-number encoding switches from 1-byte to 2-byte
/// coding at frame number 128, which changes the frame header's length and
/// therefore the byte range the header CRC-8 covers. 130 frames at a small
/// block size cheaply crosses that boundary.
#[test]
fn flac_round_trip_frame_number_utf8_boundary() {
    let sample_rate = 44100u32;
    let block_size = 16u32;
    let samples = noise_i16(7, 0.6, block_size as usize * 130);
    assert_round_trip_exact(
        &[samples],
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
        "130 frames crossing UTF-8 frame-number boundary",
    );
}

/// Every compression level exercises a different `(max_fixed_order,
/// max_lpc_order, partition_order)` combination — sweep all of them against
/// the same non-trivial signal.
#[test]
fn flac_round_trip_all_compression_levels() {
    let sample_rate = 44100u32;
    let block_size = 512u32;
    let samples = sine_i16(660.0, 0.7, sample_rate, block_size as usize * 3);

    for level in 0..=8u8 {
        let level = CompressionLevel::new(level).expect("valid level");
        assert_round_trip_exact(
            std::slice::from_ref(&samples),
            sample_rate,
            block_size,
            level,
            &format!("compression level {}", level.value()),
        );
    }
}

// ---------------------------------------------------------------------------
// CRC rejection tests (regression coverage for the new header CRC-8 check)
// ---------------------------------------------------------------------------

/// Analogous to `decoder::tests::test_flac_decode_crc_mismatch_rejects`, but
/// against a real (non-constant, FIXED/LPC-coded) frame instead of silence,
/// to confirm CRC-16 rejection still works on the code paths this mission
/// touched.
#[test]
fn flac_decode_rejects_corrupted_frame_crc16() {
    let sample_rate = 44100u32;
    let block_size = 256u32;
    let samples = sine_i16(440.0, 0.5, sample_rate, block_size as usize);

    let mut stream = encode_flac_stream(
        &[samples],
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
    );
    let len = stream.len();
    assert!(len >= 2, "encoded stream should have a CRC-16 footer");
    stream[len - 1] ^= 0xFF;
    stream[len - 2] ^= 0xFF;

    let config = AudioDecoderConfig {
        codec: CodecId::Flac,
        sample_rate,
        channels: 1,
        extradata: None,
    };
    let mut dec = FlacDecoder::new(&config).expect("decoder creation");

    // `send_packet` eagerly decodes and can surface the CRC-16 error itself
    // (it drives `try_decode_one_frame` in a loop internally); if it doesn't,
    // the error must surface from `receive_frame` instead. Either is
    // "rejected"; silently returning the corrupted frame's samples is the
    // only unacceptable outcome.
    let mut saw_rejection = dec.send_packet(&stream, 0).is_err();
    if !saw_rejection {
        loop {
            match dec.receive_frame() {
                Err(_) => {
                    saw_rejection = true;
                    break;
                }
                Ok(None) => break,
                Ok(Some(_)) => {}
            }
        }
    }
    assert!(
        saw_rejection,
        "decoder should reject a frame with a corrupted CRC-16 footer"
    );
}

/// Corrupts a header byte (not the CRC-8 byte itself) and confirms the new
/// header CRC-8 check rejects it instead of silently decoding a header the
/// encoder never wrote.
#[test]
fn flac_decode_rejects_corrupted_header_crc8() {
    let sample_rate = 44100u32;
    let block_size = 256u32;
    let samples = sine_i16(440.0, 0.5, sample_rate, block_size as usize);

    let mut stream = encode_flac_stream(
        &[samples],
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
    );

    // Locate the frame sync (0xFF 0xF8..) after the "fLaC" + STREAMINFO
    // header, then flip a bit in the byte right after the 4-byte fixed
    // header (part of the UTF-8 frame/sample number) without touching the
    // CRC-8 byte, so the corruption is only detectable via CRC-8.
    let sync_pos = stream
        .windows(2)
        .position(|w| w[0] == 0xFF && (w[1] & 0xFC) == 0xF8)
        .expect("stream should contain a frame sync");
    stream[sync_pos + 4] ^= 0x01;

    let config = AudioDecoderConfig {
        codec: CodecId::Flac,
        sample_rate,
        channels: 1,
        extradata: None,
    };
    let mut dec = FlacDecoder::new(&config).expect("decoder creation");
    dec.send_packet(&stream, 0).expect("send_packet");

    // A corrupted header must never be accepted as a valid frame with
    // corrupted PCM; it should be skipped as a false sync (Ok(None)/no
    // frames) or surfaced as an error — either way, not a "valid" decode of
    // wrong data.
    let mut frames = Vec::new();
    loop {
        match dec.receive_frame() {
            Ok(Some(f)) => frames.push(f),
            Ok(None) | Err(_) => break,
        }
    }
    assert!(
        frames.is_empty(),
        "decoder must not accept a frame whose header fails CRC-8"
    );
}

// ---------------------------------------------------------------------------
// Independent verification via the system `ffmpeg` decoder (dev-only)
// ---------------------------------------------------------------------------

/// Minimal `PATH` search for an `ffmpeg` binary — avoids adding a dev-dependency
/// just to look one up.
fn which_ffmpeg() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("ffmpeg"))
        .find(|candidate| candidate.is_file())
}

/// Cross-checks the encoder's output against the system `ffmpeg` FLAC
/// decoder — an implementation with zero knowledge of this codebase. If
/// ffmpeg accepts a file this encoder produced (including with
/// `-err_detect crccheck`) and returns exactly the source PCM, the encoder's
/// bitstream is provably spec-correct, independent of whether `FlacDecoder`
/// agrees with itself.
///
/// `#[ignore]`d deliberately: this is a dev-only sanity check, not part of
/// the `cargo nextest run -p oximedia-audio` contract, since CI/dev machines
/// are not guaranteed to have `ffmpeg` on `PATH`. Run it manually with:
///
/// ```text
/// cargo test -p oximedia-audio --test flac_roundtrip -- --ignored flac_ffmpeg_cross_check
/// ```
#[test]
#[ignore = "requires ffmpeg on PATH; not part of the default cargo nextest run"]
fn flac_ffmpeg_cross_check() {
    let Some(ffmpeg) = which_ffmpeg() else {
        eprintln!("ffmpeg not found on PATH; skipping cross-check");
        return;
    };

    let sample_rate = 44100u32;
    let block_size = 1024u32;
    // Exact multiple of block_size: sidesteps the (separate, already-tested)
    // question of whether STREAMINFO's hardcoded min=max=frame_size is a
    // faithful declaration for a ragged final block — this test is only
    // about whether ffmpeg accepts the encoder's *frame* bitstream.
    let samples = sine_i16(523.25, 0.7, sample_rate, block_size as usize * 8);

    let stream = encode_flac_stream(
        std::slice::from_ref(&samples),
        sample_rate,
        block_size,
        CompressionLevel::DEFAULT,
    );

    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "oximedia_flac_ffmpeg_check_{}.flac",
        std::process::id()
    ));
    std::fs::write(&path, &stream).expect("write temp .flac file");

    let output = std::process::Command::new(&ffmpeg)
        .args(["-v", "error", "-err_detect", "crccheck", "-i"])
        .arg(&path)
        .args(["-f", "s16le", "-"])
        .output()
        .expect("run ffmpeg");

    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "ffmpeg rejected the encoder's output:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let decoded_i16: Vec<i16> = output
        .stdout
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    assert_eq!(
        decoded_i16.len(),
        samples.len(),
        "ffmpeg decoded a different sample count than was encoded"
    );
    for (i, (&expected, &got)) in samples.iter().zip(decoded_i16.iter()).enumerate() {
        assert_eq!(
            expected, got,
            "ffmpeg decode diverges from source at sample {i}"
        );
    }
}
