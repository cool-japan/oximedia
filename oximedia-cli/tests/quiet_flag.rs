//! `--quiet` must have a real, observable effect: status/banner stdout from
//! the transcode path is suppressed, while command results (probe JSON) and
//! stderr warnings keep printing.

mod common;

use assert_cmd::Command;

/// Without `--quiet`, a WAV→FLAC transcode prints its plan and summary.
#[test]
fn transcode_without_quiet_prints_status() {
    let (dir, wav) = common::write_wav_fixture(440.0, 48_000, 1, 1.0);
    let out = dir.path().join("out_loud.flac");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "transcode",
            "-i",
            wav.to_str().expect("utf8 path"),
            "-o",
            out.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Transcode Plan"),
        "default run must print the plan, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Transcode Complete"),
        "default run must print the summary, got:\n{stdout}"
    );
    assert!(out.exists(), "output file must be written");
}

/// With `--quiet`, the same transcode emits NOTHING on stdout — but still
/// performs the real work (the output file exists).
#[test]
fn transcode_with_quiet_suppresses_status() {
    let (dir, wav) = common::write_wav_fixture(440.0, 48_000, 1, 1.0);
    let out = dir.path().join("out_quiet.flac");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "transcode",
            "-i",
            wav.to_str().expect("utf8 path"),
            "-o",
            out.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.trim().is_empty(),
        "--quiet must suppress all transcode status stdout, got:\n{stdout}"
    );
    assert!(
        out.exists(),
        "output file must still be written under --quiet"
    );
}

/// `--quiet` must NOT suppress command results: probe --format json still
/// prints its JSON payload (machine consumers rely on this).
#[test]
fn quiet_does_not_suppress_probe_json() {
    let (_dir, wav) = common::write_wav_fixture(440.0, 48_000, 1, 1.0);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "probe",
            "-i",
            wav.to_str().expect("utf8 path"),
            "--format",
            "json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("probe --format json must emit valid JSON");
    assert!(
        parsed.is_object(),
        "probe JSON payload must survive --quiet"
    );
}

/// `archive-pro migrate` (SLICE 4F quiet rollout): the human-readable
/// completion banner is decoration around the real deliverable (the
/// re-encoded file), so it follows the same suppress-under-`--quiet` rule
/// as transcode above.
#[test]
fn archivepro_migrate_without_quiet_prints_status() {
    let (dir, wav) = common::write_wav_fixture(440.0, 48_000, 1, 0.2);
    let out_dir = dir.path().join("out_loud");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "archive-pro",
            "migrate",
            "-i",
            wav.to_str().expect("utf8 path"),
            "-o",
            out_dir.to_str().expect("utf8 path"),
            "--target",
            "flac",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Archive Pro Migrate"),
        "default run must print the migrate banner, got:\n{stdout}"
    );
    assert!(out_dir.exists(), "output directory must be created");
}

#[test]
fn archivepro_migrate_with_quiet_suppresses_status() {
    let (dir, wav) = common::write_wav_fixture(440.0, 48_000, 1, 0.2);
    let out_dir = dir.path().join("out_quiet");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "archive-pro",
            "migrate",
            "-i",
            wav.to_str().expect("utf8 path"),
            "-o",
            out_dir.to_str().expect("utf8 path"),
            "--target",
            "flac",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.trim().is_empty(),
        "--quiet must suppress the migrate completion banner, got:\n{stdout}"
    );
    let migrated = out_dir
        .join(wav.file_stem().expect("stem").to_str().expect("utf8"))
        .with_extension("flac");
    assert!(
        migrated.exists(),
        "the real re-encode must still happen under --quiet"
    );
}

/// `collab` (SLICE 4F quiet rollout): `create` is a mutation whose
/// confirmation banner is decoration around the real deliverable (the
/// session record persisted to the database file).
#[test]
fn collab_create_with_quiet_suppresses_status_but_still_persists() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let db = dir.path().join("collab.json");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "collab",
            "create",
            "--project",
            "proj",
            "--name",
            "Test Session",
            "--owner",
            "alice",
            "--db",
            db.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.trim().is_empty(),
        "--quiet must suppress the session-created banner, got:\n{stdout}"
    );
    let db_contents = std::fs::read_to_string(&db).expect("db file must exist under --quiet");
    assert!(
        db_contents.contains("\"owner\": \"alice\""),
        "the session must still be persisted for real under --quiet"
    );
}

#[test]
fn collab_create_without_quiet_prints_status() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let db = dir.path().join("collab.json");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "collab",
            "create",
            "--project",
            "proj",
            "--name",
            "Test Session",
            "--owner",
            "alice",
            "--db",
            db.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Collab Session Created"),
        "default run must print the creation banner, got:\n{stdout}"
    );
}

/// `edl export` (SLICE 4F quiet rollout): the completion banner is
/// decoration around the real deliverable (the written EDL file).
#[test]
fn edl_export_quiet_suppresses_banner_but_still_writes_output() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let input = dir.path().join("in.edl");
    let output = dir.path().join("out.edl");
    std::fs::write(
        &input,
        "TITLE: Test EDL\nFCM: DROP FRAME\n\n001  AX       V     C        01:00:00:00 01:00:05:00 01:00:00:00 01:00:05:00\n\n",
    )
    .expect("write edl fixture");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "edl",
            "export",
            input.to_str().expect("utf8 path"),
            "-o",
            output.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.trim().is_empty(),
        "--quiet must suppress the EDL export banner, got:\n{stdout}"
    );
    assert!(
        output.exists(),
        "the real EDL export must still happen under --quiet"
    );
}

/// Warn-and-proceed diagnostics stay visible under `--quiet`: they go to
/// stderr, which only errors and warnings may use.
#[test]
fn quiet_keeps_stderr_warnings() {
    let (dir, wav) = common::write_wav_fixture(440.0, 48_000, 1, 1.0);
    let out = dir.path().join("out_warn.flac");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "transcode",
            "-i",
            wav.to_str().expect("utf8 path"),
            "-o",
            out.to_str().expect("utf8 path"),
            "--threads",
            "8",
        ])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("--threads"),
        "the --threads no-effect warning must reach stderr even under --quiet, got:\n{stderr}"
    );
}
