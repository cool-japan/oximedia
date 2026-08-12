//! FLAC cross-checks against external reference implementations.
//!
//! Two directions, both required for a claim of spec compliance rather than
//! mere self-consistency:
//!
//! 1. **Decode** — FLAC files produced by ffmpeg and by the libFLAC reference
//!    encoder (`tests/data/flac/`) must decode sample-exact.  These fixtures
//!    are committed, so this direction always runs.
//! 2. **Encode** — files produced by this crate must pass `flac -t` (which
//!    verifies every frame CRC-16 *and* the STREAMINFO MD5 of the decoded
//!    audio) and must decode to byte-identical PCM under ffmpeg.  This
//!    direction needs those tools on `PATH`; when they are missing the test
//!    prints an explicit skip line rather than passing silently.

use std::path::{Path, PathBuf};
use std::process::Command;

use oximedia_codec::flac::{FlacConfig, FlacDecoder, FlacEncoder};

// =============================================================================
// Direction 1 — decode reference-encoder output
// =============================================================================

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/flac")
}

/// Reference PCM as ffmpeg writes it: little-endian, `bytes` per sample.
/// 24-bit audio is carried in the high 24 bits of a 32-bit container.
fn load_reference(path: &Path, bytes: usize, shift: u32) -> Vec<i32> {
    let raw = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(raw.len() % bytes, 0, "{}: ragged PCM", path.display());
    raw.chunks_exact(bytes)
        .map(|chunk| {
            let mut le = [0u8; 4];
            le[..bytes].copy_from_slice(chunk);
            let value = i32::from_le_bytes(le);
            if bytes == 2 {
                i32::from(value as i16)
            } else {
                value >> shift
            }
        })
        .collect()
}

struct Fixture {
    name: &'static str,
    channels: u8,
    bits_per_sample: u8,
    sample_rate: u32,
    /// Bytes per sample in the reference PCM, and the right shift to apply.
    pcm_bytes: usize,
    pcm_shift: u32,
}

const FIXTURES: &[Fixture] = &[
    // ffmpeg, compression level 0 — fixed predictors throughout.
    Fixture {
        name: "mono16_cl0",
        channels: 1,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
    // ffmpeg, compression level 12 — high LPC orders and stereo decorrelation.
    Fixture {
        name: "stereo16_cl12",
        channels: 2,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
    // ffmpeg, 24-bit stereo.
    Fixture {
        name: "stereo24_cl8",
        channels: 2,
        bits_per_sample: 24,
        sample_rate: 48000,
        pcm_bytes: 4,
        pcm_shift: 8,
    },
    // libFLAC reference encoder, 192-sample blocks (uncommon block size code).
    Fixture {
        name: "mono16_bs192",
        channels: 1,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
    // libFLAC reference encoder, pink noise — verbatim/escape residual paths.
    Fixture {
        name: "stereo16_noise",
        channels: 2,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
    // libFLAC reference encoder, digital silence — constant subframes.
    Fixture {
        name: "mono16_silence",
        channels: 1,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
    // libFLAC reference encoder, full-scale white noise — incompressible, so
    // libFLAC falls back to verbatim subframes and wide Rice parameters.
    Fixture {
        name: "stereo16_verbatim",
        channels: 2,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
    // libFLAC reference encoder, samples that are all multiples of 256 —
    // exercises the wasted-bits path.
    Fixture {
        name: "mono16_wasted",
        channels: 1,
        bits_per_sample: 16,
        sample_rate: 44100,
        pcm_bytes: 2,
        pcm_shift: 0,
    },
];

#[test]
fn decodes_reference_encoder_output_sample_exact() {
    let dir = fixture_dir();
    for fixture in FIXTURES {
        let flac_path = dir.join(format!("{}.flac", fixture.name));
        let pcm_path = dir.join(format!("{}.raw", fixture.name));
        let data = std::fs::read(&flac_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", flac_path.display()));
        let expected = load_reference(&pcm_path, fixture.pcm_bytes, fixture.pcm_shift);

        let mut decoder = FlacDecoder::new();
        let decoded = decoder
            .decode_stream(&data)
            .unwrap_or_else(|e| panic!("{}: decode_stream: {e}", fixture.name));

        let info = decoder
            .stream_info()
            .unwrap_or_else(|| panic!("{}: STREAMINFO", fixture.name));
        assert_eq!(
            info.channels, fixture.channels,
            "{}: channels",
            fixture.name
        );
        assert_eq!(
            info.bits_per_sample, fixture.bits_per_sample,
            "{}: bit depth",
            fixture.name
        );
        assert_eq!(
            info.sample_rate, fixture.sample_rate,
            "{}: sample rate",
            fixture.name
        );
        assert_eq!(
            decoded.len() as u64,
            info.total_samples * u64::from(info.channels),
            "{}: STREAMINFO total samples vs decoded",
            fixture.name
        );
        assert_eq!(
            decoded.len(),
            expected.len(),
            "{}: decoded sample count",
            fixture.name
        );

        let mismatches = decoded
            .iter()
            .zip(&expected)
            .enumerate()
            .find(|(_, (a, b))| a != b);
        assert!(
            mismatches.is_none(),
            "{}: sample mismatch at {:?}",
            fixture.name,
            mismatches.map(|(i, (a, b))| (i, *a, *b))
        );
    }
}

#[test]
fn rejects_truncated_reference_file() {
    let path = fixture_dir().join("stereo16_cl12.flac");
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let truncated = &data[..data.len() - 40];
    let mut decoder = FlacDecoder::new();
    assert!(
        decoder.decode_stream(truncated).is_err(),
        "a truncated FLAC stream must not decode silently"
    );
}

// =============================================================================
// Direction 2 — encode, then verify with libFLAC and ffmpeg
// =============================================================================

/// Locate `name` on `PATH`, returning `None` when it is not installed.
fn find_tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn encode_to_file(
    path: &Path,
    sample_rate: u32,
    channels: u8,
    bits_per_sample: u8,
    block_size: usize,
    pcm: &[i32],
) {
    let config = FlacConfig {
        sample_rate,
        channels,
        bits_per_sample,
    };
    let mut encoder = FlacEncoder::with_block_size(config, block_size).expect("encoder");
    let (header, frames) = encoder.encode(pcm).expect("encode");
    let mut stream = header;
    for frame in &frames {
        stream.extend_from_slice(&frame.data);
    }
    // Rewrite STREAMINFO with the real totals and MD5, as any seekable FLAC
    // encoder does on close; this is what makes `flac -t` a full-stream check.
    stream[..42].copy_from_slice(&encoder.finalized_stream_header());
    std::fs::write(path, &stream).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Reference PCM bytes in the layout ffmpeg emits for this bit depth.
fn expected_pcm_bytes(pcm: &[i32], bits_per_sample: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 4);
    for &sample in pcm {
        if bits_per_sample <= 16 {
            out.extend_from_slice(&(sample as i16).to_le_bytes());
        } else {
            out.extend_from_slice(&(sample << 8).to_le_bytes());
        }
    }
    out
}

struct EncodeCase {
    name: &'static str,
    sample_rate: u32,
    channels: u8,
    bits_per_sample: u8,
    block_size: usize,
}

const ENCODE_CASES: &[EncodeCase] = &[
    EncodeCase {
        name: "silence_mono16",
        sample_rate: 44100,
        channels: 1,
        bits_per_sample: 16,
        block_size: 1024,
    },
    EncodeCase {
        name: "ramp_mono16",
        sample_rate: 44100,
        channels: 1,
        bits_per_sample: 16,
        block_size: 4096,
    },
    EncodeCase {
        name: "sine_stereo16",
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
        block_size: 4096,
    },
    EncodeCase {
        name: "noise_stereo16",
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
        block_size: 4096,
    },
    EncodeCase {
        name: "sine_stereo24",
        sample_rate: 48000,
        channels: 2,
        bits_per_sample: 24,
        block_size: 1024,
    },
    EncodeCase {
        name: "smallblocks_stereo16",
        sample_rate: 48000,
        channels: 2,
        bits_per_sample: 16,
        block_size: 192,
    },
];

fn case_pcm(case: &EncodeCase) -> Vec<i32> {
    let channels = case.channels as usize;
    let frames = 4000usize;
    let amplitude = ((1i64 << (case.bits_per_sample - 1)) - 1) as f64 * 0.8;
    match case.name {
        "silence_mono16" => vec![0i32; frames * channels],
        "ramp_mono16" => (0..frames * channels)
            .map(|i| ((i % 3001) as i32) - 1500)
            .collect(),
        "noise_stereo16" => {
            let mut state = 0x1234_5678u32;
            (0..frames * channels)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    (state as i32) >> 16
                })
                .collect()
        }
        _ => (0..frames * channels)
            .map(|i| {
                let sample = (i / channels) as f64;
                let channel = (i % channels) as f64;
                ((sample / f64::from(case.sample_rate)
                    * (440.0 + 37.0 * channel)
                    * std::f64::consts::TAU)
                    .sin()
                    * amplitude) as i32
            })
            .collect(),
    }
}

#[test]
fn encoder_output_passes_libflac_and_ffmpeg() {
    let flac_tool = find_tool("flac");
    let ffmpeg_tool = find_tool("ffmpeg");
    if flac_tool.is_none() && ffmpeg_tool.is_none() {
        println!(
            "SKIPPED: neither `flac` nor `ffmpeg` is on PATH, so the external \
             encode cross-check could not run. Install either to enable it."
        );
        return;
    }
    if flac_tool.is_none() {
        println!("PARTIAL: `flac` not on PATH — skipping the `flac -t` (CRC + MD5) check.");
    }
    if ffmpeg_tool.is_none() {
        println!("PARTIAL: `ffmpeg` not on PATH — skipping the byte-exact PCM comparison.");
    }

    let dir = std::env::temp_dir().join(format!(
        "oximedia-flac-external-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    for case in ENCODE_CASES {
        let pcm = case_pcm(case);
        let flac_path = dir.join(format!("{}.flac", case.name));
        encode_to_file(
            &flac_path,
            case.sample_rate,
            case.channels,
            case.bits_per_sample,
            case.block_size,
            &pcm,
        );

        // --- libFLAC: verifies every frame CRC-16 and the STREAMINFO MD5 ---
        if let Some(tool) = &flac_tool {
            let output = Command::new(tool)
                .arg("-t")
                .arg(&flac_path)
                .output()
                .unwrap_or_else(|e| panic!("{}: run flac -t: {e}", case.name));
            assert!(
                output.status.success(),
                "{}: `flac -t` rejected our stream:\n{}",
                case.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // --- ffmpeg: byte-exact PCM ---------------------------------------
        if let Some(tool) = &ffmpeg_tool {
            let pcm_path = dir.join(format!("{}.pcm", case.name));
            let format = if case.bits_per_sample <= 16 {
                "s16le"
            } else {
                "s32le"
            };
            let output = Command::new(tool)
                .args(["-y", "-v", "error", "-i"])
                .arg(&flac_path)
                .args(["-f", format])
                .arg(&pcm_path)
                .output()
                .unwrap_or_else(|e| panic!("{}: run ffmpeg: {e}", case.name));
            assert!(
                output.status.success(),
                "{}: ffmpeg failed to decode our stream:\n{}",
                case.name,
                String::from_utf8_lossy(&output.stderr)
            );
            let produced = std::fs::read(&pcm_path).expect("read decoded PCM");
            let expected = expected_pcm_bytes(&pcm, case.bits_per_sample);
            assert_eq!(
                produced.len(),
                expected.len(),
                "{}: ffmpeg PCM length",
                case.name
            );
            let first_diff = produced.iter().zip(&expected).position(|(a, b)| a != b);
            assert!(
                first_diff.is_none(),
                "{}: ffmpeg PCM differs at byte {:?}",
                case.name,
                first_diff
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Variable-block-size streams (irregular chunk sizes, explicit sample numbers
/// in every frame header) must also be accepted by the reference tools.
#[test]
fn variable_block_size_output_passes_libflac_and_ffmpeg() {
    let flac_tool = find_tool("flac");
    let ffmpeg_tool = find_tool("ffmpeg");
    if flac_tool.is_none() && ffmpeg_tool.is_none() {
        println!(
            "SKIPPED: neither `flac` nor `ffmpeg` is on PATH, so the variable-block-size \
             cross-check could not run."
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!("oximedia-flac-varblock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let flac_path = dir.join("varblock.flac");

    let config = FlacConfig {
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };
    let mut encoder = FlacEncoder::with_block_size(config, 4096).expect("encoder");
    encoder
        .set_variable_block_size(true)
        .expect("settable before encoding");

    let mut pcm: Vec<i32> = Vec::new();
    let mut stream: Vec<u8> = Vec::new();
    let mut header_written = false;
    for &count in &[700usize, 4096, 55, 6000, 1] {
        let chunk: Vec<i32> = (0..count * 2)
            .map(|i| {
                let sample = ((i + pcm.len()) / 2) as f64;
                ((sample / 44100.0 * 523.25 * std::f64::consts::TAU).sin() * 21000.0) as i32
            })
            .collect();
        let (header, frames) = encoder.encode(&chunk).expect("encode chunk");
        if !header_written {
            stream.extend_from_slice(&header);
            header_written = true;
        }
        for frame in &frames {
            stream.extend_from_slice(&frame.data);
        }
        pcm.extend_from_slice(&chunk);
    }
    stream[..42].copy_from_slice(&encoder.finalized_stream_header());
    std::fs::write(&flac_path, &stream).expect("write stream");

    if let Some(tool) = &flac_tool {
        let output = Command::new(tool)
            .arg("-t")
            .arg(&flac_path)
            .output()
            .expect("run flac -t");
        assert!(
            output.status.success(),
            "`flac -t` rejected our variable-block stream:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        println!("PARTIAL: `flac` not on PATH — skipped the CRC + MD5 check.");
    }

    if let Some(tool) = &ffmpeg_tool {
        let pcm_path = dir.join("varblock.pcm");
        let output = Command::new(tool)
            .args(["-y", "-v", "error", "-i"])
            .arg(&flac_path)
            .args(["-f", "s16le"])
            .arg(&pcm_path)
            .output()
            .expect("run ffmpeg");
        assert!(
            output.status.success(),
            "ffmpeg rejected our variable-block stream:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(&pcm_path).expect("read PCM"),
            expected_pcm_bytes(&pcm, 16),
            "ffmpeg PCM must match the source exactly"
        );
    } else {
        println!("PARTIAL: `ffmpeg` not on PATH — skipped the byte-exact PCM comparison.");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A stream we encode must survive a full round trip through the reference
/// decoder *and* back through our own decoder with identical results.
#[test]
fn encoder_output_decodes_identically_here_and_externally() {
    let Some(ffmpeg_tool) = find_tool("ffmpeg") else {
        println!("SKIPPED: `ffmpeg` not on PATH — cannot compare against an external decoder.");
        return;
    };

    let dir = std::env::temp_dir().join(format!("oximedia-flac-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let flac_path = dir.join("parity.flac");
    let pcm_path = dir.join("parity.pcm");

    let pcm: Vec<i32> = (0..8000 * 2)
        .map(|i| {
            let sample = (i / 2) as f64;
            let channel = (i % 2) as f64;
            ((sample / 44100.0 * (523.25 + channel) * std::f64::consts::TAU).sin() * 24000.0
                + (sample / 44100.0 * 1567.98 * std::f64::consts::TAU).sin() * 4000.0)
                as i32
        })
        .collect();
    encode_to_file(&flac_path, 44100, 2, 16, 4096, &pcm);

    let output = Command::new(&ffmpeg_tool)
        .args(["-y", "-v", "error", "-i"])
        .arg(&flac_path)
        .args(["-f", "s16le"])
        .arg(&pcm_path)
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let external = load_reference(&pcm_path, 2, 0);

    let data = std::fs::read(&flac_path).expect("read our stream");
    let mut decoder = FlacDecoder::new();
    let ours = decoder.decode_stream(&data).expect("decode our stream");

    assert_eq!(ours, pcm, "our decoder must be lossless");
    assert_eq!(external, pcm, "ffmpeg must decode our stream losslessly");
    assert_eq!(ours, external, "both decoders must agree exactly");

    let _ = std::fs::remove_dir_all(&dir);
}
