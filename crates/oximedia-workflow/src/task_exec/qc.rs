//! Real quality control for
//! [`TaskType::QualityControl`](crate::task::TaskType::QualityControl).
//!
//! The task's `profile` selects an `oximedia-qc` [`QcPreset`] and the rule set
//! it installs; `oximedia_qc::QualityControl::validate` then probes the file
//! and runs every applicable rule. The task fails when the report does.
//!
//! # Why the container is checked first
//!
//! `oximedia-qc`'s file probe only reads real stream metadata for the
//! containers it recognises by magic bytes (ISO-BMFF, Matroska/WebM, AVI,
//! WAV, FLAC, Ogg, MXF). For anything else it falls back to *synthesised*
//! stream information, so a "passed" verdict would describe invented
//! metadata rather than the file. [`assert_qc_probeable`] therefore rejects
//! such inputs up front with an honest error instead of reporting a QC
//! result that was never really measured.
//!
//! # Required rules
//!
//! `rules` names checks that **must** have run. Each entry is matched against
//! the `rule_name` of the produced check results (see the `name()` values in
//! `oximedia_qc::{video, audio, container, compliance}`, e.g.
//! `audio_codec_validation`, `loudness_compliance`, `format_validation`). A
//! requested rule that the profile never evaluated fails the task, because a
//! workflow that asked for a check must not be told it passed.
//!
//! Note that `oximedia-qc` filters its installed rules through
//! `QcRule::is_applicable`, so a rule the profile installs may still not run
//! on a particular file (a video rule on an audio-only master, say). Naming
//! such a rule in `rules` therefore fails the task by design — list a rule
//! only when the check is genuinely required for that media.

use std::io::Read;
use std::path::Path;

use tracing::{debug, info};

use oximedia_qc::{QcPreset, QualityControl};

use crate::error::{Result, WorkflowError};
use crate::task_exec::{verify_non_empty_file, TaskOutcome};

/// Accepted QC profile names, used in error messages.
const ACCEPTED_PROFILES: &str = "basic, streaming, broadcast, comprehensive, youtube, vimeo";

/// Resolves a workflow QC profile name onto an `oximedia-qc` preset.
///
/// Hyphens, underscores and case are ignored (`ultra_broadcast` is still
/// rejected — only the six real presets are accepted).
///
/// # Errors
///
/// Returns [`WorkflowError::InvalidParameter`] for an unknown profile, naming
/// every accepted value. Unknown profiles are **not** silently downgraded to
/// a default preset: a workflow must never be told a profile ran when a
/// different one did.
pub fn resolve_profile(profile: &str) -> Result<QcPreset> {
    let key = profile.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match key.as_str() {
        "basic" => Ok(QcPreset::Basic),
        "streaming" => Ok(QcPreset::Streaming),
        "broadcast" => Ok(QcPreset::Broadcast),
        "comprehensive" | "" => Ok(QcPreset::Comprehensive),
        "youtube" => Ok(QcPreset::YouTube),
        "vimeo" => Ok(QcPreset::Vimeo),
        other => Err(WorkflowError::InvalidParameter {
            param: "profile".to_string(),
            value: format!("{other} (accepted QC profiles: {ACCEPTED_PROFILES})"),
        }),
    }
}

/// Container formats whose stream metadata `oximedia-qc` really parses.
fn qc_probeable_container(magic: &[u8]) -> Option<&'static str> {
    if magic.len() >= 8 {
        match &magic[4..8] {
            b"ftyp" => return Some("ISO-BMFF (MP4/MOV)"),
            b"moov" | b"mdat" | b"free" | b"wide" | b"skip" => return Some("ISO-BMFF (MP4)"),
            _ => {}
        }
    }
    if magic.len() >= 4 {
        if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
            return Some("Matroska/WebM");
        }
        if &magic[..4] == b"fLaC" {
            return Some("FLAC");
        }
        if &magic[..4] == b"OggS" {
            return Some("Ogg");
        }
        if magic[..4] == [0x06, 0x0E, 0x2B, 0x34] {
            return Some("MXF");
        }
    }
    if magic.len() >= 12 && &magic[..4] == b"RIFF" {
        return match &magic[8..12] {
            b"AVI " => Some("AVI"),
            b"WAVE" => Some("WAV"),
            _ => None,
        };
    }
    None
}

/// Rejects inputs for which `oximedia-qc` would synthesise stream metadata.
///
/// # Errors
///
/// Returns an error when the file cannot be read, or when its container is
/// not one that `oximedia-qc` genuinely probes.
pub fn assert_qc_probeable(input: &Path) -> Result<&'static str> {
    let mut file = std::fs::File::open(input).map_err(|e| {
        WorkflowError::generic(format!("Cannot open QC input {}: {e}", input.display()))
    })?;
    let mut magic = [0u8; 16];
    let read = file.read(&mut magic).map_err(|e| {
        WorkflowError::generic(format!("Cannot read QC input {}: {e}", input.display()))
    })?;

    qc_probeable_container(&magic[..read]).ok_or_else(|| {
        WorkflowError::generic(format!(
            "QC cannot honestly validate {}: oximedia-qc only parses real stream metadata for \
             ISO-BMFF (MP4/MOV), Matroska/WebM, AVI, WAV, FLAC, Ogg and MXF; for other \
             containers it synthesises stream information, so any verdict would describe \
             invented metadata rather than this file",
            input.display()
        ))
    })
}

/// Runs the QC rules of `profile` over `input`.
///
/// Blocking, rayon-parallel rule evaluation runs on a Tokio blocking thread.
///
/// # Errors
///
/// Returns an error when the input is missing/empty, the container is not
/// genuinely probeable, the profile is unknown, a required rule did not run,
/// or the report contains failing checks.
pub async fn execute_quality_control(
    input: &Path,
    profile: &str,
    rules: &[String],
) -> Result<TaskOutcome> {
    if !input.exists() {
        return Err(WorkflowError::FileNotFound(input.to_path_buf()));
    }
    let input_size = verify_non_empty_file(input, "QC input")?;
    let container = assert_qc_probeable(input)?;
    let preset = resolve_profile(profile)?;

    let path_string = input.to_string_lossy().to_string();
    let report = {
        let owned = path_string.clone();
        tokio::task::spawn_blocking(move || QualityControl::with_preset(preset).validate(&owned))
            .await
            .map_err(|e| WorkflowError::generic(format!("QC task failed: {e}")))?
            .map_err(|e| {
                WorkflowError::generic(format!("QC validation of {path_string} failed: {e}"))
            })?
    };

    debug!(
        "QC {} ({container}, profile {profile}): {}/{} checks passed",
        input.display(),
        report.passed_checks,
        report.total_checks
    );

    // Every rule the caller explicitly demanded must actually have run.
    let evaluated: Vec<String> = {
        let mut names: Vec<String> = report.results.iter().map(|r| r.rule_name.clone()).collect();
        names.sort_unstable();
        names.dedup();
        names
    };
    let missing: Vec<&String> = rules
        .iter()
        .filter(|requested| !evaluated.iter().any(|name| name == *requested))
        .collect();
    if !missing.is_empty() {
        return Err(WorkflowError::generic(format!(
            "QC profile `{profile}` never evaluated required rule(s) {missing:?} on {}; \
             rules actually run: {evaluated:?}",
            input.display()
        )));
    }

    let failures: Vec<String> = report
        .critical_errors()
        .into_iter()
        .chain(report.errors())
        .map(|r| format!("{}: {}", r.rule_name, r.message))
        .collect();
    let warnings: Vec<String> = report
        .warnings()
        .into_iter()
        .map(|r| format!("{}: {}", r.rule_name, r.message))
        .collect();

    if !report.overall_passed {
        return Err(WorkflowError::generic(format!(
            "QC failed for {} (profile `{profile}`): {}",
            input.display(),
            if failures.is_empty() {
                format!(
                    "{} of {} checks failed",
                    report.failed_checks, report.total_checks
                )
            } else {
                failures.join("; ")
            }
        )));
    }

    info!(
        "QC passed for {} (profile `{profile}`, {} checks, {} warnings)",
        input.display(),
        report.total_checks,
        warnings.len()
    );

    Ok(TaskOutcome::with_data(serde_json::json!({
        "kind": "quality_control",
        "input": input.display().to_string(),
        "input_bytes": input_size,
        "container": container,
        "profile": profile,
        "total_checks": report.total_checks,
        "passed_checks": report.passed_checks,
        "failed_checks": report.failed_checks,
        "required_rules": rules,
        "rules_evaluated": evaluated,
        "warnings": warnings,
        "validation_duration_secs": report.validation_duration,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia_wf_qc_{}_{name}", std::process::id()))
    }

    fn wav_bytes(sample_count: usize) -> Vec<u8> {
        let data_size = (sample_count * 2) as u32;
        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&48_000u32.to_le_bytes());
        buf.extend_from_slice(&96_000u32.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for i in 0..sample_count {
            let v = ((i as f64 * 0.05).sin() * 12_000.0) as i16;
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf
    }

    #[test]
    fn known_profiles_resolve() {
        assert!(matches!(resolve_profile("basic"), Ok(QcPreset::Basic)));
        assert!(matches!(
            resolve_profile("Broadcast"),
            Ok(QcPreset::Broadcast)
        ));
        assert!(resolve_profile("you-tube").is_err());
        assert!(matches!(resolve_profile("youtube"), Ok(QcPreset::YouTube)));
        assert!(matches!(resolve_profile(""), Ok(QcPreset::Comprehensive)));
    }

    #[test]
    fn unknown_profile_is_rejected_not_defaulted() {
        let err = resolve_profile("super-broadcast").expect_err("unknown profile");
        let message = err.to_string();
        assert!(message.contains("super_broadcast"), "{message}");
        assert!(message.contains("comprehensive"), "{message}");
    }

    #[test]
    fn probeable_containers_are_recognised() {
        assert_eq!(qc_probeable_container(b"RIFF\0\0\0\0WAVEfmt "), Some("WAV"));
        assert_eq!(qc_probeable_container(b"RIFF\0\0\0\0AVI LIST"), Some("AVI"));
        assert_eq!(
            qc_probeable_container(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0]),
            Some("Matroska/WebM")
        );
        assert_eq!(
            qc_probeable_container(b"\0\0\0\x20ftypisom"),
            Some("ISO-BMFF (MP4/MOV)")
        );
        assert_eq!(qc_probeable_container(b"YUV4MPEG2 W16"), None);
    }

    #[tokio::test]
    async fn y4m_input_is_refused_because_qc_would_fabricate_metadata() {
        let path = temp_path("fabricate.y4m");
        std::fs::write(&path, b"YUV4MPEG2 W16 H16 F25:1 Ip A1:1 C420jpeg\nFRAME\n").expect("write");

        let err = execute_quality_control(&path, "basic", &[])
            .await
            .expect_err("Y4M must be refused");
        assert!(err.to_string().contains("synthesises stream information"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn missing_input_reports_file_not_found() {
        let path = temp_path("absent.wav");
        let _ = std::fs::remove_file(&path);
        let err = execute_quality_control(&path, "basic", &[])
            .await
            .expect_err("missing input");
        assert!(matches!(err, WorkflowError::FileNotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn requested_rule_that_never_ran_fails_the_task() {
        let path = temp_path("required_rule.wav");
        std::fs::write(&path, wav_bytes(4_800)).expect("write wav");

        let err =
            execute_quality_control(&path, "basic", &["a_rule_that_does_not_exist".to_string()])
                .await
                .expect_err("missing rule must fail the task");
        let message = err.to_string();
        assert!(message.contains("a_rule_that_does_not_exist"), "{message}");
        assert!(message.contains("rules actually run"), "{message}");

        let _ = std::fs::remove_file(path);
    }
}
