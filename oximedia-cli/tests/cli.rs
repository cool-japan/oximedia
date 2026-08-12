//! Core end-to-end smoke tests for the `oximedia` binary itself.
//!
//! This file is the primary "does the binary work at all" gate: version
//! reporting, top-level help, invalid-subcommand handling, and a real
//! `probe` run against a tiny synthetic media file written to
//! `std::env::temp_dir()`. It intentionally overlaps only lightly with the
//! more granular suites (`cli_help.rs`, `cli_help_per_command.rs`,
//! `probe_json_snapshot.rs`, `exit_code_smoke.rs`) — those cover exhaustive
//! per-subcommand and per-flag detail; this file is the fast, high-signal
//! sanity check.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

/// Current workspace version (`version.workspace = true` in Cargo.toml).
///
/// Sourced from `CARGO_PKG_VERSION` rather than hand-written: this integration
/// test is compiled as part of the `oximedia-cli` package, which is the same
/// package that owns the `oximedia` binary, so the constant tracks the
/// workspace version automatically across a `/bump` instead of silently
/// drifting out of date (it was pinned at `0.2.0` through the 0.2.1 bump).
const EXPECTED_VERSION: &str = env!("CARGO_PKG_VERSION");

fn oximedia() -> Command {
    Command::cargo_bin("oximedia").expect("oximedia binary should exist")
}

// ---------------------------------------------------------------------------
// --version
// ---------------------------------------------------------------------------

#[test]
fn version_flag_reports_current_version() {
    oximedia()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("oximedia"))
        .stdout(predicate::str::contains(EXPECTED_VERSION));
}

#[test]
fn version_subcommand_reports_current_version() {
    // `oximedia version` is a richer, hand-written report (not the clap
    // auto-generated `--version` flag) — verify it agrees with Cargo.toml.
    oximedia()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains(EXPECTED_VERSION));
}

#[test]
fn version_subcommand_json_reports_current_version() {
    let output = oximedia()
        .args(["version", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(output).expect("version --json output should be UTF-8");
    let v: serde_json::Value =
        serde_json::from_str(&s).expect("version --json should produce valid JSON");
    assert_eq!(
        v.get("version").and_then(|x| x.as_str()),
        Some(EXPECTED_VERSION),
        "version JSON field should match workspace version; got: {s}"
    );
}

// ---------------------------------------------------------------------------
// --help
// ---------------------------------------------------------------------------

#[test]
fn help_flag_lists_key_subcommands() {
    // Assert on stable substrings only — never on exact wording/formatting,
    // which churns as clap's help renderer and our own descriptions evolve.
    let out = oximedia()
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);

    for needle in ["Usage", "probe", "transcode", "extract", "batch", "info"] {
        assert!(
            s.to_lowercase().contains(&needle.to_lowercase()),
            "--help output should mention `{needle}`; got:\n{s}"
        );
    }
}

#[test]
fn help_short_flag_matches_long_flag() {
    // `-h` and `--help` should both exit 0 (clap wires both by default).
    oximedia().arg("-h").assert().success();
}

// ---------------------------------------------------------------------------
// Invalid subcommand handling
// ---------------------------------------------------------------------------

#[test]
fn invalid_subcommand_fails_with_helpful_stderr() {
    oximedia()
        .arg("this-subcommand-does-not-exist")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error").or(predicate::str::contains("unrecognized")))
        // clap's "did you mean" / usage guidance should still show up so the
        // user isn't left with a bare, unhelpful failure.
        .stderr(predicate::str::contains("Usage").or(predicate::str::contains("usage")));
}

#[test]
fn invalid_subcommand_does_not_panic() {
    let output = oximedia()
        .arg("this-subcommand-does-not-exist")
        .output()
        .expect("failed to run oximedia");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "invalid subcommand must not panic: {stderr}"
    );
    assert!(!output.status.success());
}

// ---------------------------------------------------------------------------
// `probe` against a tiny synthetic media file (hermetic, no network)
// ---------------------------------------------------------------------------

/// Build a minimal-but-valid WAV file: 44-byte RIFF/WAVE header plus a
/// handful of silent PCM frames. Small enough to be instant, but a real,
/// parseable container so `probe` exercises its actual format-detection
/// path rather than an early-return error.
fn make_tiny_wav() -> Vec<u8> {
    let sample_rate: u32 = 8_000;
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let num_samples: u32 = 80; // 10ms of audio — deliberately tiny
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_size = num_samples * u32::from(channels) * u32::from(bits_per_sample / 8);
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.resize(buf.len() + data_size as usize, 0); // silence
    buf
}

/// Writes the fixture into a fresh, per-call [`TempDir`] (hermetic, no
/// repo-tree pollution, no filename collisions between tests that run in
/// parallel within the same process) and returns `(TempDir, path)`. The
/// `TempDir` must be kept alive for the duration of the test — dropping it
/// deletes the directory.
fn write_tiny_wav_fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create TempDir");
    let path = dir.path().join("fixture.wav");
    std::fs::write(&path, make_tiny_wav()).expect("failed to write tiny WAV fixture");
    (dir, path)
}

#[test]
fn probe_tiny_wav_fixture_succeeds_with_sensible_output() {
    let (_dir, path) = write_tiny_wav_fixture();

    let output = oximedia()
        .args([
            "--quiet",
            "probe",
            "-i",
            path.to_str().expect("fixture path is UTF-8"),
        ])
        .output()
        .expect("failed to run oximedia probe");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "probe must not panic on a tiny valid WAV file: {stderr}"
    );
    assert!(
        output.status.success(),
        "probe should succeed on a tiny valid WAV file; stderr: {stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "probe should print something for a valid file"
    );
}

#[test]
fn probe_tiny_wav_fixture_json_is_well_formed() {
    let (_dir, path) = write_tiny_wav_fixture();

    let output = oximedia()
        .args([
            "--quiet",
            "probe",
            "-i",
            path.to_str().expect("fixture path is UTF-8"),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run oximedia probe --format json");

    assert!(
        output.status.success(),
        "probe --format json should succeed on a tiny valid WAV file; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("probe stdout should be UTF-8");
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("probe --format json output should be valid JSON");
    assert!(
        v.is_object(),
        "probe JSON output should be a JSON object; got: {v}"
    );
}

#[test]
fn probe_missing_file_fails_cleanly() {
    let missing = std::env::temp_dir()
        .join(format!(
            "oximedia_cli_probe_missing_{}.wav",
            std::process::id()
        ))
        .display()
        .to_string();

    let output = oximedia()
        .args(["probe", "-i", &missing])
        .output()
        .expect("failed to run oximedia probe on missing file");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "probe on a missing file must not panic: {stderr}"
    );
    assert!(
        !output.status.success(),
        "probe on a missing file should exit non-zero"
    );
}
