//! Honesty tests for `subtitle extract` / `subtitle burn` / `timecode burn`.
//!
//! `subtitle extract` delegates to the real Matroska/WebM subtitle demux
//! shared with `captions extract`. `subtitle burn` and `timecode burn` are
//! both implemented for real now, on the Y4M frame harness — here each must
//! refuse a non-Y4M input with the shared actionable error rather than
//! fabricating an output file; their real burn-in pixel coverage lives in
//! `frame_harness_e2e.rs`.

use assert_cmd::Command;
use std::path::Path;

/// Encode `n` (0..=127) as a 1-byte EBML VINT size marker.
fn ebml_size1(n: usize) -> u8 {
    assert!(
        n <= 0x7F,
        "fixture value too large for 1-byte EBML VINT: {n}"
    );
    0x80 | (n as u8)
}

/// Build a minimal, valid WebM byte buffer with a single `S_TEXT/UTF8`
/// subtitle track carrying `cues` (cluster-relative timecode in ms, text).
///
/// Byte-for-byte the same EBML structure as the fixture used by the captions
/// extract unit tests (which mirrors `oximedia_container`'s own Matroska
/// demuxer fixture).
fn build_test_webm_subtitle(cues: &[(i16, &str)]) -> Vec<u8> {
    let codec_id = "S_TEXT/UTF8";
    let mut data = Vec::new();

    // EBML header (DocType "webm").
    data.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F]);
    data.extend_from_slice(&[0x42, 0x86, 0x81, 0x01]); // EBMLVersion
    data.extend_from_slice(&[0x42, 0xF7, 0x81, 0x01]); // EBMLReadVersion
    data.extend_from_slice(&[0x42, 0xF2, 0x81, 0x04]); // EBMLMaxIDLength
    data.extend_from_slice(&[0x42, 0xF3, 0x81, 0x08]); // EBMLMaxSizeLength
    data.extend_from_slice(&[0x42, 0x82, 0x84, b'w', b'e', b'b', b'm']); // DocType
    data.extend_from_slice(&[0x42, 0x87, 0x81, 0x04]); // DocTypeVersion
    data.extend_from_slice(&[0x42, 0x85, 0x81, 0x02]); // DocTypeReadVersion

    // Segment (unbounded size).
    data.extend_from_slice(&[
        0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);

    // Info: TimecodeScale = 1_000_000 ns (1 ms per unit).
    data.extend_from_slice(&[0x15, 0x49, 0xA9, 0x66, ebml_size1(7)]);
    data.extend_from_slice(&[0x2A, 0xD7, 0xB1, ebml_size1(3), 0x0F, 0x42, 0x40]);

    // Tracks → one subtitle TrackEntry.
    let codec_bytes = codec_id.as_bytes();
    let codec_id_elem_len = 2 + codec_bytes.len();
    let track_entry_content_len = 3 + 5 + 3 + codec_id_elem_len;

    let mut track_entry = Vec::new();
    track_entry.push(0xAE); // TrackEntry ID
    track_entry.push(ebml_size1(track_entry_content_len));
    track_entry.extend_from_slice(&[0xD7, 0x81, 0x01]); // TrackNumber: 1
    track_entry.extend_from_slice(&[0x73, 0xC5, 0x82, 0x30, 0x39]); // TrackUID
    track_entry.extend_from_slice(&[0x83, 0x81, 0x11]); // TrackType: subtitle
    track_entry.push(0x86); // CodecID ID
    track_entry.push(ebml_size1(codec_bytes.len()));
    track_entry.extend_from_slice(codec_bytes);

    data.extend_from_slice(&[0x16, 0x54, 0xAE, 0x6B]); // Tracks ID
    data.push(ebml_size1(track_entry.len()));
    data.extend_from_slice(&track_entry);

    // Cluster (unbounded) + Timestamp(0) + one SimpleBlock per cue.
    data.extend_from_slice(&[
        0x1F, 0x43, 0xB6, 0x75, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    data.extend_from_slice(&[0xE7, 0x81, 0x00]); // Cluster Timestamp: 0

    for &(timecode, text) in cues {
        let text_bytes = text.as_bytes();
        let block_content_len = 1 + 2 + 1 + text_bytes.len();
        assert!(block_content_len <= 0x7F, "test cue too long");
        data.push(0xA3); // SimpleBlock ID
        data.push(ebml_size1(block_content_len));
        data.push(0x81); // track number VINT = 1
        data.extend_from_slice(&timecode.to_be_bytes());
        data.push(0x80); // flags: keyframe
        data.extend_from_slice(text_bytes);
    }

    data
}

fn write_srt(path: &Path) {
    std::fs::write(path, "1\n00:00:01,000 --> 00:00:02,000\nHello burn\n\n")
        .expect("write srt fixture");
}

/// `subtitle extract` performs a real Matroska demux and writes real cues.
#[test]
fn subtitle_extract_real_webm_track() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let input = dir.path().join("subs.webm");
    let output = dir.path().join("subs_out.srt");
    std::fs::write(
        &input,
        build_test_webm_subtitle(&[(0, "Hello world"), (2000, "Second cue")]),
    )
    .expect("write webm fixture");

    Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "subtitle",
            "extract",
            "-i",
            input.to_str().expect("utf8"),
            "-o",
            output.to_str().expect("utf8"),
        ])
        .assert()
        .success();

    let srt = std::fs::read_to_string(&output).expect("extracted SRT must exist");
    assert!(
        srt.contains("Hello world") && srt.contains("Second cue"),
        "extracted SRT must contain the real cue text, got:\n{srt}"
    );
    assert!(
        srt.contains("-->"),
        "extracted SRT must contain real cue timing lines, got:\n{srt}"
    );
}

/// Non-Matroska input fails honestly and writes nothing.
#[test]
fn subtitle_extract_non_matroska_errors() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let input = dir.path().join("not_media.txt");
    let output = dir.path().join("never_written.srt");
    std::fs::write(&input, b"plain text, not a container").expect("write fixture");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "subtitle",
            "extract",
            "-i",
            input.to_str().expect("utf8"),
            "-o",
            output.to_str().expect("utf8"),
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("Matroska/WebM"),
        "error must name the supported containers, got:\n{stderr}"
    );
    assert!(!output.exists(), "no output may be fabricated on failure");
}

/// `subtitle burn` is real now (see `frame_harness_e2e.rs`), but its input
/// contract is Y4M in / Y4M out: a WebM input is refused with the shared,
/// actionable "convert it first" error, and no output file is fabricated —
/// mirroring `timecode_burn_errors_honestly` below.
#[test]
fn subtitle_burn_errors_honestly() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let video = dir.path().join("video.webm");
    let srt = dir.path().join("subs.srt");
    let output = dir.path().join("burned.webm");
    std::fs::write(&video, build_test_webm_subtitle(&[(0, "x")])).expect("write video");
    write_srt(&srt);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "subtitle",
            "burn",
            "-i",
            video.to_str().expect("utf8"),
            "--subtitle",
            srt.to_str().expect("utf8"),
            "-o",
            output.to_str().expect("utf8"),
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("YUV4MPEG2"),
        "subtitle burn must state its Y4M input requirement, got:\n{stderr}"
    );
    assert!(
        stderr.contains("oximedia transcode"),
        "the error must tell the user how to convert, got:\n{stderr}"
    );
    assert!(!output.exists(), "burn must not fabricate an output file");
}

/// `timecode burn` is real now (see `frame_harness_e2e.rs`), but its input
/// contract is Y4M in / Y4M out: a WebM input is refused with the shared,
/// actionable "convert it first" error, and no output file is fabricated.
#[test]
fn timecode_burn_errors_honestly() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let video = dir.path().join("video.webm");
    let output = dir.path().join("tc_burned.webm");
    std::fs::write(&video, build_test_webm_subtitle(&[(0, "x")])).expect("write video");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "timecode",
            "burn",
            "-i",
            video.to_str().expect("utf8"),
            "-o",
            output.to_str().expect("utf8"),
            "--start",
            "01:00:00:00",
            "--fps",
            "25",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("YUV4MPEG2"),
        "timecode burn must state its Y4M input requirement, got:\n{stderr}"
    );
    assert!(
        stderr.contains("oximedia transcode"),
        "the error must tell the user how to convert, got:\n{stderr}"
    );
    assert!(
        !output.exists(),
        "timecode burn must not fabricate an output file"
    );
}

/// Parameter validation still fires first: a bad position is its own error,
/// reported before the input format and the font are even looked at.
#[test]
fn timecode_burn_still_validates_parameters() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let video = dir.path().join("video.webm");
    std::fs::write(&video, b"stub-bytes").expect("write video");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "timecode",
            "burn",
            "-i",
            video.to_str().expect("utf8"),
            "-o",
            dir.path().join("out.webm").to_str().expect("utf8"),
            "--position",
            "middle-ish",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("Invalid position"),
        "parameter validation must precede the unimplemented error, got:\n{stderr}"
    );
}
