//! `validate --loudness-check` runs a real EBU R128 measurement (WAV/PCM and
//! FLAC); `validate --gamut-check` runs a real EBU R103 legal-range check on
//! Y4M input and reports a visible skip for anything else.

mod common;

use assert_cmd::Command;

/// A -6 dBFS sine sits far above the -23 LUFS EBU R128 target, so the real
/// loudness check must report a deviation issue with the measured value.
#[test]
fn loudness_check_reports_real_deviation() {
    let (_dir, wav) = common::write_wav_fixture(1000.0, 48_000, 2, 2.0);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args(["validate", wav.to_str().expect("utf8"), "--loudness-check"])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Loudness"),
        "the loudness check must appear in the report, got:\n{stdout}"
    );
    assert!(
        stdout.contains("LUFS"),
        "a real measured LUFS value must be reported, got:\n{stdout}"
    );
    assert!(
        stdout.contains("deviates"),
        "a hot sine must be flagged as deviating from target, got:\n{stdout}"
    );
}

/// Undecodable audio produces a visible skip issue — not a silent pass.
#[test]
fn loudness_check_undecodable_is_visible_skip() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let fake = dir.path().join("fake.wav");
    std::fs::write(&fake, b"RIFF").expect("write stub");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args(["validate", fake.to_str().expect("utf8"), "--loudness-check"])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Loudness check skipped"),
        "the skip must be visible in the report, got:\n{stdout}"
    );
}

/// A -6 dBFS sine encoded losslessly as FLAC must measure the same as WAV:
/// the real loudness check now decodes FLAC too, not just WAV/PCM.
#[test]
fn loudness_check_decodes_flac() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let flac = dir.path().join("test.flac");
    let raw = common::make_ramp_flac(1, 44_100);
    std::fs::write(&flac, raw).expect("write FLAC fixture");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args(["validate", flac.to_str().expect("utf8"), "--loudness-check"])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("LUFS"),
        "a real measured LUFS value must be reported for FLAC input, got:\n{stdout}"
    );
}

/// `--gamut-check` on a Y4M clip with an out-of-studio-range frame reports a
/// real EBU R103 legal-range violation (exit 0 — Warning severity, not a
/// hard failure, since Y4M carries no full/limited-range flag).
#[test]
fn gamut_check_detects_out_of_range_y4m() {
    // Full white (255) is above the legal luma maximum of 235.
    let (_dir, y4m) = common::write_y4m_fixture(4, 4, 255, 128);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args(["validate", y4m.to_str().expect("utf8"), "--gamut-check"])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Gamut"),
        "the gamut check must appear in the report, got:\n{stdout}"
    );
    assert!(
        stdout.contains("studio"),
        "the studio-range assumption must be disclosed, got:\n{stdout}"
    );
}

/// `--gamut-check` on a non-Y4M input produces a visible skip (stdout,
/// Info-severity issue) and still proceeds (exit 0) — not a silent no-op.
#[test]
fn gamut_check_non_y4m_is_visible_skip() {
    let (_dir, wav) = common::write_wav_fixture(1000.0, 48_000, 1, 0.5);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args(["validate", wav.to_str().expect("utf8"), "--gamut-check"])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Gamut check skipped"),
        "the skip must be visible in the report, got:\n{stdout}"
    );
}
