//! `distributed start-coordinator`/`status` real-config-and-honesty tests
//! (SLICE 4F).
//!
//! `--max-workers` previously had nowhere real to go (only a warning said
//! so); it is now threaded into a real `oximedia_distributed::coordinator::
//! CoordinatorConfig`. `--watch` previously warned and silently fell
//! through to a single query; it now refuses with a real error naming the
//! stub gRPC client, since no real coordinator transport exists yet.

use assert_cmd::Command;

/// `--max-workers` is genuinely threaded into the coordinator's config and
/// reported back verbatim, rather than being silently dropped.
#[test]
fn max_workers_explicit_value_is_honored_and_reported() {
    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "distributed",
            "start-coordinator",
            "--bind",
            "127.0.0.1:0",
            "--max-workers",
            "3",
            "--json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(value["max_workers"], 3, "got: {stdout}");
    assert_eq!(value["max_workers_explicit"], true, "got: {stdout}");
}

/// Without `--max-workers`, the coordinator's real default (from
/// `CoordinatorConfig::default()`) is reported, not a fabricated
/// "unlimited" placeholder.
#[test]
fn max_workers_default_is_the_real_coordinator_default() {
    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "distributed",
            "start-coordinator",
            "--bind",
            "127.0.0.1:0",
            "--json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(
        value["max_workers"], 1000,
        "must report CoordinatorConfig::default().max_workers, got: {stdout}"
    );
    assert_eq!(value["max_workers_explicit"], false, "got: {stdout}");
}

/// `--data-dir` still has nowhere real to go; it must warn on stderr rather
/// than silently doing nothing.
#[test]
fn data_dir_still_warns_on_stderr() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "distributed",
            "start-coordinator",
            "--bind",
            "127.0.0.1:0",
            "--data-dir",
            dir.path().to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("--data-dir"),
        "must warn that --data-dir is not implemented, got:\n{stderr}"
    );
}

/// An invalid `--bind` address is a real error, not a silently-ignored one.
#[test]
fn invalid_bind_address_is_a_real_error() {
    Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "distributed",
            "start-coordinator",
            "--bind",
            "not-an-address",
        ])
        .assert()
        .failure();
}

/// `--watch` refuses with a real, descriptive error instead of silently
/// falling through to a single unwatched query.
#[test]
fn watch_flag_is_an_honest_error_not_a_silent_single_query() {
    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "distributed",
            "status",
            "--coordinator",
            "127.0.0.1:19999",
            "--watch",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("CoordinatorServiceClient") || stderr.contains("--watch"),
        "error must explain why --watch cannot work yet, got:\n{stderr}"
    );
}

/// `start-coordinator`'s human-readable banner is decoration around a real
/// side effect (the background server task); `--quiet` suppresses it.
#[test]
fn start_coordinator_quiet_suppresses_banner() {
    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--quiet",
            "distributed",
            "start-coordinator",
            "--bind",
            "127.0.0.1:0",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.trim().is_empty(),
        "--quiet must suppress the coordinator banner, got:\n{stdout}"
    );
}

#[test]
fn start_coordinator_without_quiet_prints_banner() {
    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args(["distributed", "start-coordinator", "--bind", "127.0.0.1:0"])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(
        stdout.contains("Distributed Coordinator"),
        "default run must print the coordinator banner, got:\n{stdout}"
    );
}
