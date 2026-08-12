//! FLAC encoder → decoder round-trip tests.
//!
//! Every assertion here is exact: the decoder must consume precisely the bytes
//! the encoder declared, and reproduce every sample bit-for-bit.  Cross-checks
//! against libFLAC and ffmpeg live in `flac_external.rs`; this file pins the
//! in-tree pair against itself across signals, block sizes, bit depths and
//! channel layouts.

use oximedia_codec::flac::frame::{ChannelAssignment, FrameHeader};
use oximedia_codec::flac::{bitio::BitReader, FlacConfig, FlacDecoder, FlacEncoder};

// =============================================================================
// Signal generators
// =============================================================================

fn silence(n: usize, channels: usize, _depth: u32) -> Vec<i32> {
    vec![0i32; n * channels]
}

fn ramp(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let span = 1i64 << (depth - 2);
    (0..n * channels)
        .map(|i| {
            let s = (i / channels) as i64;
            let c = (i % channels) as i64;
            (((s * (7 + c)) % (2 * span)) - span) as i32
        })
        .collect()
}

fn sine(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let amp = ((1i64 << (depth - 1)) - 1) as f64 * 0.85;
    (0..n * channels)
        .map(|i| {
            let s = (i / channels) as f64;
            let c = (i % channels) as f64;
            ((s / 44100.0 * (440.0 + 37.0 * c) * std::f64::consts::TAU).sin() * amp) as i32
        })
        .collect()
}

fn two_tone(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let amp = ((1i64 << (depth - 1)) - 1) as f64 * 0.45;
    (0..n * channels)
        .map(|i| {
            let s = (i / channels) as f64;
            let c = (i % channels) as f64;
            let t = s / 48000.0;
            ((t * (330.0 + c) * std::f64::consts::TAU).sin() * amp
                + (t * (1731.0 + 5.0 * c) * std::f64::consts::TAU).sin() * amp) as i32
        })
        .collect()
}

fn noise(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let mut state = 0x2545_F491u32;
    let shift = 32 - depth;
    (0..n * channels)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as i32) >> shift
        })
        .collect()
}

/// Alternating full-scale extremes — the worst case for predictor range.
fn square_full_scale(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let hi = ((1i64 << (depth - 1)) - 1) as i32;
    let lo = -(1i64 << (depth - 1)) as i32;
    (0..n * channels)
        .map(|i| if (i / channels) % 2 == 0 { lo } else { hi })
        .collect()
}

/// Mostly silent with sparse impulses — exercises high Rice parameters.
fn impulses(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let hi = ((1i64 << (depth - 1)) - 1) as i32;
    (0..n * channels)
        .map(|i| {
            let s = i / channels;
            if s % 97 == 0 {
                if s % 194 == 0 {
                    hi
                } else {
                    -hi
                }
            } else {
                0
            }
        })
        .collect()
}

/// Every sample has a run of zero low bits — the wasted-bits path.
fn wasted_bits(n: usize, channels: usize, depth: u32) -> Vec<i32> {
    let shift = (depth / 2).clamp(1, 8);
    let span = 1i64 << (depth - shift - 1);
    (0..n * channels)
        .map(|i| {
            let s = (i / channels) as i64;
            (((((s * 13) % (2 * span)) - span) << shift) as i32).max(-(1 << (depth - 1)))
        })
        .collect()
}

type Generator = fn(usize, usize, u32) -> Vec<i32>;

const GENERATORS: &[(&str, Generator)] = &[
    ("silence", silence),
    ("ramp", ramp),
    ("sine", sine),
    ("two_tone", two_tone),
    ("noise", noise),
    ("square_full_scale", square_full_scale),
    ("impulses", impulses),
    ("wasted_bits", wasted_bits),
];

// =============================================================================
// Helpers
// =============================================================================

/// Encode `pcm`, decode it back frame by frame, and assert exactness.
///
/// Returns the channel assignments the encoder chose, one per frame.
fn round_trip(
    label: &str,
    pcm: &[i32],
    sample_rate: u32,
    channels: u8,
    bits_per_sample: u8,
    block_size: usize,
) -> Vec<ChannelAssignment> {
    let config = FlacConfig {
        sample_rate,
        channels,
        bits_per_sample,
    };
    let mut encoder =
        FlacEncoder::with_block_size(config, block_size).expect("block size must be valid");
    let (header, frames) = encoder
        .encode(pcm)
        .unwrap_or_else(|e| panic!("{label}: {e}"));

    let expected_frames = pcm.len() / channels as usize;
    let expected_frames = expected_frames.div_ceil(block_size);
    assert_eq!(frames.len(), expected_frames, "{label}: frame count");

    // --- Frame-by-frame decode ------------------------------------------
    // Parse the metadata first so the decoder knows the stream block size and
    // can number a short final frame correctly.
    let mut decoder = FlacDecoder::new();
    decoder
        .parse_metadata(&header)
        .unwrap_or_else(|e| panic!("{label}: metadata: {e}"));
    let mut decoded: Vec<i32> = Vec::with_capacity(pcm.len());
    let mut assignments = Vec::with_capacity(frames.len());
    let mut expected_sample_number = 0u64;
    for (index, frame) in frames.iter().enumerate() {
        let mut reader = BitReader::new(&frame.data);
        let parsed = FrameHeader::parse(&mut reader)
            .unwrap_or_else(|e| panic!("{label}: frame header must parse: {e}"));
        assignments.push(parsed.channel_assignment);
        assert_eq!(
            parsed.block_size, frame.block_size,
            "{label}: header block size"
        );

        let (block, consumed) = decoder
            .decode_frame(&frame.data)
            .unwrap_or_else(|e| panic!("{label}: decode: {e}"));
        assert_eq!(
            consumed,
            frame.data.len(),
            "{label}: decoder must consume exactly the {} bytes written",
            frame.data.len()
        );
        assert_eq!(block.channels, channels as usize, "{label}: channel count");
        assert_eq!(
            block.block_size, frame.block_size as usize,
            "{label}: block size"
        );
        assert_eq!(
            block.sample_number, expected_sample_number,
            "{label}: sample number"
        );
        assert_eq!(
            block.frame_number,
            Some(index as u64),
            "{label}: frame number"
        );
        assert_eq!(
            frame.sample_number, expected_sample_number,
            "{label}: encoder-reported sample number"
        );
        assert_eq!(
            block.samples.len(),
            block.block_size * block.channels,
            "{label}: sample buffer size"
        );
        expected_sample_number += block.block_size as u64;
        decoded.extend_from_slice(&block.samples);
    }
    assert_eq!(decoded.len(), pcm.len(), "{label}: total sample count");
    assert_eq!(decoded, pcm, "{label}: FLAC must be lossless");

    // --- Whole-stream decode --------------------------------------------
    let mut stream = header;
    for frame in &frames {
        stream.extend_from_slice(&frame.data);
    }
    stream[..42].copy_from_slice(&encoder.finalized_stream_header());

    let mut stream_decoder = FlacDecoder::new();
    let stream_samples = stream_decoder
        .decode_stream(&stream)
        .unwrap_or_else(|e| panic!("{label}: decode_stream: {e}"));
    assert_eq!(
        stream_samples, pcm,
        "{label}: decode_stream must be lossless"
    );

    let info = stream_decoder
        .stream_info()
        .unwrap_or_else(|| panic!("{label}: STREAMINFO must be parsed"));
    assert_eq!(info.sample_rate, sample_rate, "{label}: STREAMINFO rate");
    assert_eq!(info.channels, channels, "{label}: STREAMINFO channels");
    assert_eq!(
        info.bits_per_sample, bits_per_sample,
        "{label}: STREAMINFO depth"
    );
    assert_eq!(
        info.total_samples,
        (pcm.len() / channels as usize) as u64,
        "{label}: STREAMINFO total samples"
    );

    assignments
}

// =============================================================================
// Tests
// =============================================================================

#[test]
fn mono_16_bit_round_trips_every_signal_and_block_size() {
    for &(name, generate) in GENERATORS {
        for &block_size in &[192usize, 256, 1024, 4096] {
            let pcm = generate(2500, 1, 16);
            round_trip(
                &format!("mono16 {name} bs={block_size}"),
                &pcm,
                44100,
                1,
                16,
                block_size,
            );
        }
    }
}

#[test]
fn stereo_16_bit_round_trips_every_signal() {
    for &(name, generate) in GENERATORS {
        let pcm = generate(3000, 2, 16);
        round_trip(&format!("stereo16 {name}"), &pcm, 44100, 2, 16, 1024);
    }
}

#[test]
fn twenty_four_bit_round_trips_mono_and_stereo() {
    for &(name, generate) in GENERATORS {
        for &channels in &[1u8, 2] {
            let pcm = generate(2048, channels as usize, 24);
            round_trip(
                &format!("24-bit {name} ch={channels}"),
                &pcm,
                48000,
                channels,
                24,
                1024,
            );
        }
    }
}

#[test]
fn eight_and_twenty_bit_depths_round_trip() {
    for &depth in &[8u8, 12, 20] {
        for &(name, generate) in GENERATORS {
            let pcm = generate(1500, 2, u32::from(depth));
            round_trip(&format!("{depth}-bit {name}"), &pcm, 44100, 2, depth, 1024);
        }
    }
}

#[test]
fn uncommon_block_sizes_round_trip() {
    // Block sizes outside the frame header's table force the uncommon 8-bit
    // and 16-bit forms, which store `block_size - 1`.
    for &block_size in &[1usize, 2, 17, 100, 255, 257, 999, 4097, 8000] {
        let pcm = sine(block_size * 2 + 3, 2, 16);
        round_trip(
            &format!("uncommon bs={block_size}"),
            &pcm,
            44100,
            2,
            16,
            block_size,
        );
    }
}

#[test]
fn short_final_frame_round_trips() {
    // 4096-sample blocks with 5000 samples: the last frame is 904 samples.
    let pcm = two_tone(5000, 2, 16);
    round_trip("short final frame", &pcm, 44100, 2, 16, 4096);
}

#[test]
fn multichannel_round_trips() {
    for channels in 3u8..=8 {
        let pcm = two_tone(1024, channels as usize, 16);
        round_trip(&format!("{channels}ch"), &pcm, 48000, channels, 16, 512);
    }
}

#[test]
fn every_stereo_decorrelation_mode_is_emitted_and_inverted() {
    let mut seen: Vec<ChannelAssignment> = Vec::new();

    // Independent: uncorrelated noise in each channel.
    let mut state = 0x9E37_79B9u32;
    let independent: Vec<i32> = (0..4096 * 2)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as i32) >> 17
        })
        .collect();
    seen.extend(round_trip("independent", &independent, 44100, 2, 16, 1024));

    // Mid/side: L = tone + noise, R = tone - noise.  Each channel on its own is
    // noise-dominated and expensive, but the mid is the bare tone (cheap) and
    // the side is twice the noise, so mid/side beats every other assignment.
    let mut noise_state = 0x1357_9BDFu32;
    let mid_side: Vec<i32> = (0..4096)
        .flat_map(|i| {
            noise_state ^= noise_state << 13;
            noise_state ^= noise_state >> 17;
            noise_state ^= noise_state << 5;
            let tone = ((f64::from(i) * 0.013).sin() * 20000.0) as i32;
            let noise = (noise_state as i32) >> 19;
            [tone + noise, tone - noise]
        })
        .collect();
    seen.extend(round_trip("mid_side", &mid_side, 44100, 2, 16, 1024));

    // Left/side: a quiet, simple left channel and a noisy right one.
    let left_side: Vec<i32> = (0..4096)
        .flat_map(|i| {
            let l = ((f64::from(i) * 0.01).sin() * 8000.0) as i32;
            let r = l - ((f64::from(i) * 0.37).sin() * 15000.0) as i32;
            [l, r]
        })
        .collect();
    seen.extend(round_trip("left_side", &left_side, 44100, 2, 16, 1024));

    // Side/right: the mirror case.
    let side_right: Vec<i32> = (0..4096)
        .flat_map(|i| {
            let r = ((f64::from(i) * 0.01).sin() * 8000.0) as i32;
            let l = r + ((f64::from(i) * 0.41).sin() * 15000.0) as i32;
            [l, r]
        })
        .collect();
    seen.extend(round_trip("side_right", &side_right, 44100, 2, 16, 1024));

    // Every one of the four assignments must actually be selected somewhere
    // above, or that encoder path has no end-to-end bitstream coverage.
    for expected in [
        ChannelAssignment::Independent(2),
        ChannelAssignment::LeftSide,
        ChannelAssignment::SideRight,
        ChannelAssignment::MidSide,
    ] {
        assert!(
            seen.contains(&expected),
            "{expected:?} was never emitted; saw {:?}",
            {
                let mut distinct: Vec<ChannelAssignment> = Vec::new();
                for assignment in &seen {
                    if !distinct.contains(assignment) {
                        distinct.push(*assignment);
                    }
                }
                distinct
            }
        );
    }
}

#[test]
fn extreme_sample_values_round_trip() {
    // The full 16-bit and 24-bit ranges, including the asymmetric minimum.
    for &(depth, lo, hi) in &[
        (16u8, i32::from(i16::MIN), i32::from(i16::MAX)),
        (24, -(1 << 23), (1 << 23) - 1),
    ] {
        let pcm: Vec<i32> = (0..1024)
            .flat_map(|i| match i % 4 {
                0 => [lo, hi],
                1 => [hi, lo],
                2 => [0, -1],
                _ => [lo / 2, hi / 2],
            })
            .collect();
        round_trip(&format!("extremes {depth}-bit"), &pcm, 44100, 2, depth, 256);
    }
}

#[test]
fn dc_offset_round_trips() {
    for &value in &[1i32, -1, 1000, -32768, 32767] {
        let pcm = vec![value; 2048];
        round_trip(&format!("dc {value}"), &pcm, 44100, 1, 16, 1024);
    }
}

/// Feed whole blocks across several calls: the normal streaming pattern.
#[test]
fn streaming_whole_blocks_round_trips() {
    let config = FlacConfig {
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };
    let mut encoder = FlacEncoder::with_block_size(config, 1024).expect("encoder");
    let mut expected: Vec<i32> = Vec::new();
    let mut stream: Vec<u8> = Vec::new();

    for chunk in 0..4 {
        let pcm: Vec<i32> = (0..1024 * 2)
            .map(|i| (((i + chunk * 512) % 4001) as i32) - 2000)
            .collect();
        let (header, frames) = encoder.encode(&pcm).expect("encode chunk");
        if chunk == 0 {
            stream.extend_from_slice(&header);
        }
        for frame in &frames {
            stream.extend_from_slice(&frame.data);
        }
        expected.extend_from_slice(&pcm);
    }
    stream[..42].copy_from_slice(&encoder.finalized_stream_header());

    let mut decoder = FlacDecoder::new();
    let decoded = decoder.decode_stream(&stream).expect("decode_stream");
    assert_eq!(decoded, expected, "streamed encode must be lossless");
    assert_eq!(encoder.samples_encoded(), 4096);
}

/// A fixed-block-size stream codes frame *numbers*, so a short frame in the
/// middle would silently misplace every following frame.  The encoder must
/// refuse rather than emit a stream whose sample positions are wrong — a defect
/// neither `flac -t` nor a lossless round-trip would catch, because only
/// seeking depends on it.
#[test]
fn fixed_block_size_rejects_a_short_non_final_frame() {
    let config = FlacConfig {
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };
    let mut encoder = FlacEncoder::with_block_size(config, 4096).expect("encoder");
    let chunk: Vec<i32> = (0..1000 * 2).map(|i| ((i % 601) as i32) - 300).collect();

    // The first partial block is fine — it may still turn out to be the last.
    let (_, frames) = encoder.encode(&chunk).expect("first chunk");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].block_size, 1000);

    // A second call would need a frame numbered 1, i.e. starting at sample
    // 4096, when it really starts at sample 1000.
    let err = encoder
        .encode(&chunk)
        .expect_err("a short non-final frame must be refused");
    let message = format!("{err}");
    assert!(
        message.contains("short non-final frame"),
        "error must explain the constraint, got: {message}"
    );

    // The strategy is locked once frames exist.
    assert!(
        encoder.set_variable_block_size(true).is_err(),
        "the blocking strategy must not change mid-stream"
    );
}

/// Variable block sizes code an explicit sample number, so irregular chunk
/// sizes are represented exactly.
#[test]
fn variable_block_size_handles_irregular_chunks() {
    let config = FlacConfig {
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };
    let mut encoder = FlacEncoder::with_block_size(config, 4096).expect("encoder");
    encoder
        .set_variable_block_size(true)
        .expect("strategy is settable before encoding");
    assert!(encoder.is_variable_block_size());

    let mut expected: Vec<i32> = Vec::new();
    let mut stream: Vec<u8> = Vec::new();
    let mut all_frames = Vec::new();
    let mut header_written = false;

    for &count in &[1000usize, 4096, 37, 9000, 1] {
        let pcm: Vec<i32> = (0..count * 2)
            .map(|i| (((i + expected.len()) % 4001) as i32) - 2000)
            .collect();
        let (header, frames) = encoder.encode(&pcm).expect("encode irregular chunk");
        if !header_written {
            stream.extend_from_slice(&header);
            header_written = true;
        }
        for frame in &frames {
            stream.extend_from_slice(&frame.data);
            all_frames.push((frame.sample_number, frame.block_size));
        }
        expected.extend_from_slice(&pcm);
    }
    stream[..42].copy_from_slice(&encoder.finalized_stream_header());

    let mut decoder = FlacDecoder::new();
    let decoded = decoder.decode_stream(&stream).expect("decode_stream");
    assert_eq!(decoded, expected, "variable-block stream must be lossless");

    // Every frame must report its true sample position.
    let frame_decoder = FlacDecoder::new();
    let mut offset = 42usize;
    let mut position = 0u64;
    for &(encoder_sample_number, block_size) in &all_frames {
        assert_eq!(encoder_sample_number, position, "encoder sample number");
        let (block, consumed) = frame_decoder
            .decode_frame(&stream[offset..])
            .expect("decode frame");
        assert_eq!(
            block.sample_number, position,
            "variable-block frames must code an exact sample number"
        );
        assert_eq!(
            block.frame_number, None,
            "variable blocks have no frame number"
        );
        assert_eq!(block.block_size, block_size as usize);
        position += u64::from(block_size);
        offset += consumed;
    }
    assert_eq!(offset, stream.len(), "every frame must be accounted for");
}

#[test]
fn compression_actually_compresses() {
    // Not a correctness gate, but a regression guard: a tonal signal must not
    // fall back to verbatim coding.
    let pcm = two_tone(8192, 2, 16);
    let config = FlacConfig {
        sample_rate: 48000,
        channels: 2,
        bits_per_sample: 16,
    };
    let mut encoder = FlacEncoder::with_block_size(config, 4096).expect("encoder");
    let (_, frames) = encoder.encode(&pcm).expect("encode");
    let coded: usize = frames.iter().map(|f| f.data.len()).sum();
    let raw = pcm.len() * 2;
    assert!(
        coded * 2 < raw,
        "expected better than 2:1 on a two-tone signal, got {coded} vs {raw} bytes"
    );
}

#[test]
fn corrupted_frames_are_rejected() {
    let pcm = sine(2048, 2, 16);
    let config = FlacConfig {
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };
    let mut encoder = FlacEncoder::with_block_size(config, 1024).expect("encoder");
    let (_, frames) = encoder.encode(&pcm).expect("encode");
    let decoder = FlacDecoder::new();

    // A flipped payload bit must fail the frame CRC-16.
    let mut corrupted = frames[0].data.clone();
    let index = corrupted.len() / 2;
    corrupted[index] ^= 0x01;
    assert!(
        decoder.decode_frame(&corrupted).is_err(),
        "a corrupted frame body must not decode silently"
    );

    // A flipped header bit must fail the header CRC-8.
    let mut corrupted = frames[0].data.clone();
    corrupted[3] ^= 0x10;
    assert!(
        decoder.decode_frame(&corrupted).is_err(),
        "a corrupted frame header must not decode silently"
    );

    // Truncation must be an error, never a short read.
    let truncated = &frames[0].data[..frames[0].data.len() / 2];
    assert!(
        decoder.decode_frame(truncated).is_err(),
        "a truncated frame must not decode"
    );
}
