//! Task execution engine
//!
//! [`TaskExecutor`] itself, its lifecycle (register/execute/deregister),
//! and the QC/analysis/fingerprint task bodies live here. The transcode and
//! thumbnail task bodies are large enough (real pipeline/codec plumbing) to
//! push this file over the workspace's 2000-line-per-file policy, so they
//! are split into sibling `impl TaskExecutor` blocks in `task_transcode.rs`
//! and `task_thumbnail.rs` — Rust allows a type's inherent methods to span
//! multiple `impl` blocks across files within one crate.

use super::media;
use crate::{FarmError, Result, TaskId};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Result of task execution
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: TaskId,
    pub success: bool,
    pub output: Vec<u8>,
    pub duration_ms: u64,
    pub error_message: Option<String>,
    pub metrics: HashMap<String, String>,
}

/// Task execution context
#[derive(Debug)]
struct TaskContext {
    #[allow(dead_code)]
    task_id: TaskId,
    #[allow(dead_code)]
    start_time: Instant,
    progress: f64,
}

/// Maximum audio duration (seconds) analysed by [`TaskExecutor::execute_fingerprint`].
/// See that method's rustdoc and [`TaskExecutor::fingerprint_window_len`] for why.
const MAX_FINGERPRINT_SECS: f64 = 120.0;
/// Task executor
pub struct TaskExecutor {
    active_tasks: Arc<RwLock<HashMap<TaskId, TaskContext>>>,
}

impl TaskExecutor {
    /// Create a new task executor
    #[must_use]
    pub fn new() -> Self {
        Self {
            active_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Execute a task
    pub async fn execute(&self, task_id: TaskId, payload: Vec<u8>) -> Result<TaskResult> {
        // Register task as active
        {
            let mut active_tasks = self.active_tasks.write();
            if active_tasks.contains_key(&task_id) {
                return Err(FarmError::AlreadyExists(format!(
                    "Task {task_id} is already running"
                )));
            }

            active_tasks.insert(
                task_id,
                TaskContext {
                    task_id,
                    start_time: Instant::now(),
                    progress: 0.0,
                },
            );
        }

        tracing::info!("Starting execution of task {}", task_id);

        // Execute the task
        let result = self.execute_task_impl(task_id, payload).await;

        // Remove from active tasks
        {
            let mut active_tasks = self.active_tasks.write();
            active_tasks.remove(&task_id);
        }

        result
    }

    /// Internal task execution implementation
    async fn execute_task_impl(&self, task_id: TaskId, payload: Vec<u8>) -> Result<TaskResult> {
        let start = Instant::now();

        // Parse task payload
        let task_spec = self.parse_task_payload(&payload)?;

        // Execute based on task type
        let output = match task_spec.task_type.as_str() {
            "transcode" => self.execute_transcode(task_id, &task_spec).await?,
            "thumbnail" => self.execute_thumbnail(task_id, &task_spec).await?,
            "qc" => self.execute_qc(task_id, &task_spec).await?,
            "analysis" => self.execute_analysis(task_id, &task_spec).await?,
            "fingerprint" => self.execute_fingerprint(task_id, &task_spec).await?,
            _ => {
                return Err(FarmError::Task(format!(
                    "Unknown task type: {}",
                    task_spec.task_type
                )))
            }
        };

        let duration = start.elapsed();

        Ok(TaskResult {
            task_id,
            success: true,
            output,
            duration_ms: duration.as_millis() as u64,
            error_message: None,
            metrics: HashMap::new(),
        })
    }

    /// Parse task payload
    fn parse_task_payload(&self, payload: &[u8]) -> Result<TaskSpecification> {
        if payload.is_empty() {
            // Default task specification for empty payload
            return Ok(TaskSpecification {
                task_type: "transcode".to_string(),
                input_path: "/input/default.mp4".to_string(),
                output_path: "/output/default.mp4".to_string(),
                parameters: HashMap::new(),
            });
        }

        // In a real implementation, this would deserialize the payload
        serde_json::from_slice(payload).map_err(FarmError::from)
    }
    /// Parses an optional string-valued task parameter into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] naming the offending key/value when the
    /// parameter is present but fails to parse.
    pub(super) fn parse_opt_param<T>(
        params: &HashMap<String, String>,
        key: &str,
    ) -> Result<Option<T>>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        match params.get(key) {
            None => Ok(None),
            Some(raw) => raw
                .trim()
                .parse::<T>()
                .map(Some)
                .map_err(|e| FarmError::Task(format!("invalid `{key}` value '{raw}': {e}"))),
        }
    }
    /// Execute QC validation task.
    ///
    /// Runs real `oximedia-qc` rules against `spec.input_path` (an optional
    /// `qc_profile`/`profile` entry in `spec.parameters` selects the preset;
    /// see [`Self::resolve_qc_preset`]). The container is checked against
    /// [`media::assert_qc_probeable`] first: `oximedia-qc` synthesises fake
    /// stream metadata for containers it does not recognise by magic bytes,
    /// so trusting its verdict on those would report a check that never
    /// really happened.
    ///
    /// Task **success** here means the checks genuinely ran to completion —
    /// it does *not* mean the content passed them. The QC verdict itself
    /// (`overall_passed`, `total_checks`, `failed_checks`, per-rule results)
    /// is returned as JSON in the task output for the caller to act on;
    /// distinguishing "QC could not be performed" (an `Err`) from "QC ran
    /// and found problems" (an `Ok` whose payload says so) keeps that
    /// judgement with whoever consumes the report instead of baking a single
    /// pass/fail policy into the worker.
    async fn execute_qc(&self, task_id: TaskId, spec: &TaskSpecification) -> Result<Vec<u8>> {
        tracing::info!("Running QC on {}", spec.input_path);
        self.update_progress(task_id, 0.1);

        let container = media::assert_qc_probeable(&spec.input_path).await?;
        let preset = Self::resolve_qc_preset(&spec.parameters)?;
        self.update_progress(task_id, 0.3);

        let path = spec.input_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            oximedia_qc::QualityControl::with_preset(preset).validate(&path)
        })
        .await
        .map_err(|e| FarmError::Task(format!("QC task join error: {e}")))?
        .map_err(|e| {
            FarmError::Task(format!(
                "QC validation failed for '{}': {e}",
                spec.input_path
            ))
        })?;

        self.update_progress(task_id, 1.0);

        let results: Vec<serde_json::Value> = report
            .results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "rule": r.rule_name,
                    "passed": r.passed,
                    "severity": r.severity.to_string(),
                    "message": r.message,
                })
            })
            .collect();

        let payload = serde_json::json!({
            "kind": "quality_control",
            "input": spec.input_path,
            "container": container,
            "overall_passed": report.overall_passed,
            "total_checks": report.total_checks,
            "passed_checks": report.passed_checks,
            "failed_checks": report.failed_checks,
            "validation_duration_secs": report.validation_duration,
            "results": results,
        });

        serde_json::to_vec(&payload)
            .map_err(|e| FarmError::Task(format!("failed to serialize QC report: {e}")))
    }

    /// Resolves an optional `qc_profile`/`profile` task parameter onto an
    /// `oximedia-qc` preset, defaulting to [`oximedia_qc::QcPreset::Basic`]
    /// when neither key is present.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] for an unrecognised profile name — an
    /// unknown profile is never silently downgraded to a default, since a
    /// caller that asked for a specific preset must not be told a different
    /// one ran.
    fn resolve_qc_preset(parameters: &HashMap<String, String>) -> Result<oximedia_qc::QcPreset> {
        let Some(raw) = parameters
            .get("qc_profile")
            .or_else(|| parameters.get("profile"))
        else {
            return Ok(oximedia_qc::QcPreset::Basic);
        };
        let key = raw.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match key.as_str() {
            "basic" | "" => Ok(oximedia_qc::QcPreset::Basic),
            "streaming" => Ok(oximedia_qc::QcPreset::Streaming),
            "broadcast" => Ok(oximedia_qc::QcPreset::Broadcast),
            "comprehensive" => Ok(oximedia_qc::QcPreset::Comprehensive),
            "youtube" => Ok(oximedia_qc::QcPreset::YouTube),
            "vimeo" => Ok(oximedia_qc::QcPreset::Vimeo),
            other => Err(FarmError::Task(format!(
                "unknown qc_profile '{other}' (accepted: basic, streaming, broadcast, \
                 comprehensive, youtube, vimeo)"
            ))),
        }
    }

    /// Execute media analysis task.
    ///
    /// Decodes `spec.input_path` as WAV/PCM (see [`media::decode_wav`]) and
    /// runs real spectral/pitch/dynamics/transient analysis via
    /// `oximedia-audio-analysis`. Any other container returns an honest
    /// error naming the detected bytes instead of a fabricated result;
    /// video-domain analysis (`oximedia-analysis`) is not wired into this
    /// task yet.
    async fn execute_analysis(&self, task_id: TaskId, spec: &TaskSpecification) -> Result<Vec<u8>> {
        tracing::info!("Analyzing {}", spec.input_path);
        self.update_progress(task_id, 0.1);

        let audio = media::decode_wav(&spec.input_path).await?;
        self.update_progress(task_id, 0.4);

        let mono = audio.to_mono();
        let sample_rate = audio.sample_rate;
        let sample_count = mono.len();
        let duration_secs = audio.duration_secs();
        let path = spec.input_path.clone();

        let summary = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
            let analyzer = oximedia_audio_analysis::AudioAnalyzer::new(
                oximedia_audio_analysis::AnalysisConfig::default(),
            );
            let result = analyzer
                .analyze(&mono, sample_rate as f32)
                .map_err(|e| FarmError::Task(format!("audio analysis failed for '{path}': {e}")))?;

            Ok(serde_json::json!({
                "kind": "analysis",
                "input": path,
                "sample_rate": sample_rate,
                "sample_count": sample_count,
                "duration_secs": duration_secs,
                "spectral": {
                    "centroid_hz": result.spectral.centroid,
                    "flatness": result.spectral.flatness,
                    "crest": result.spectral.crest,
                    "bandwidth_hz": result.spectral.bandwidth,
                    "rolloff_hz": result.spectral.rolloff,
                    "flux": result.spectral.flux,
                },
                "pitch": {
                    "mean_f0_hz": result.pitch.mean_f0,
                    "voicing_rate": result.pitch.voicing_rate,
                },
                "dynamics": {
                    "peak": result.dynamics.peak,
                    "rms": result.dynamics.rms,
                    "crest": result.dynamics.crest,
                    "dynamic_range_db": result.dynamics.dynamic_range_db,
                    "loudness_variation": result.dynamics.loudness_variation,
                },
                "transients": {
                    "count": result.transients.num_transients,
                    "avg_strength": result.transients.avg_strength,
                },
                "voiced": result.voice.is_some(),
            }))
        })
        .await
        .map_err(|e| FarmError::Task(format!("analysis task join error: {e}")))??;

        self.update_progress(task_id, 1.0);

        serde_json::to_vec(&summary)
            .map_err(|e| FarmError::Task(format!("failed to serialize analysis summary: {e}")))
    }

    /// Execute fingerprinting task.
    ///
    /// Decodes `spec.input_path` as WAV/PCM (see [`media::decode_wav`]),
    /// downmixes to mono, and computes a real Chromaprint/AcoustID-style
    /// fingerprint via `oximedia_mir::fingerprint::AcoustidEncoder`. Any
    /// other container returns an honest error rather than a fabricated
    /// fingerprint. See [`Self::execute_fingerprint_capped`] for the
    /// analysis-window cap this applies.
    async fn execute_fingerprint(
        &self,
        task_id: TaskId,
        spec: &TaskSpecification,
    ) -> Result<Vec<u8>> {
        self.execute_fingerprint_capped(task_id, spec, MAX_FINGERPRINT_SECS)
            .await
    }

    /// Implementation of [`Self::execute_fingerprint`], parameterised on the
    /// analysis-window cap (in seconds) so tests can exercise real
    /// truncation behaviour without paying for a multi-second/minute
    /// fingerprint computation. Production code always calls this through
    /// [`Self::execute_fingerprint`] with [`MAX_FINGERPRINT_SECS`].
    async fn execute_fingerprint_capped(
        &self,
        task_id: TaskId,
        spec: &TaskSpecification,
        max_secs: f64,
    ) -> Result<Vec<u8>> {
        tracing::info!("Fingerprinting {}", spec.input_path);
        self.update_progress(task_id, 0.1);

        let audio = media::decode_wav(&spec.input_path).await?;
        self.update_progress(task_id, 0.4);

        let mono_full = audio.to_mono();
        let sample_rate = audio.sample_rate;
        let input_duration_secs = audio.duration_secs();
        let (window_len, truncated) =
            Self::fingerprint_window_len(mono_full.len(), sample_rate, max_secs);
        let mono = if truncated {
            mono_full[..window_len].to_vec()
        } else {
            mono_full
        };
        let sample_count = mono.len();

        // Chroma extraction is an O(frame_size^2) per-frame DFT (see
        // `oximedia_mir::fingerprint::acoustid::ChromaExtractor`): measured
        // at ~2.7s of wall time per 10s of 48kHz mono audio in a release
        // build (~28s in an unoptimised/debug build), hence both the
        // blocking-thread dispatch and the analysis-window cap above.
        let fingerprint = tokio::task::spawn_blocking(move || {
            oximedia_mir::fingerprint::AcoustidEncoder::compute(&mono, sample_rate)
        })
        .await
        .map_err(|e| FarmError::Task(format!("fingerprint task join error: {e}")))?;

        self.update_progress(task_id, 1.0);

        if fingerprint.is_empty() {
            return Err(FarmError::Task(format!(
                "fingerprint extraction produced no hashes for '{}' ({sample_count} samples @ \
                 {sample_rate} Hz is too short for the fingerprint frame window)",
                spec.input_path
            )));
        }

        let payload = serde_json::json!({
            "kind": "fingerprint",
            "input": spec.input_path,
            "sample_rate": sample_rate,
            "channels": audio.channels,
            "input_duration_secs": input_duration_secs,
            "fingerprinted_secs": fingerprint.duration_secs,
            "truncated_to_analysis_window": truncated,
            "hash_count": fingerprint.len(),
            "fingerprint": fingerprint.fingerprint,
        });

        serde_json::to_vec(&payload)
            .map_err(|e| FarmError::Task(format!("failed to serialize fingerprint: {e}")))
    }

    /// Clamps a sample count to at most `max_secs` seconds at `sample_rate`,
    /// returning `(window_len, was_truncated)`.
    ///
    /// Real Chromaprint/AcoustID tooling (`fpcalc`) defaults to analysing at
    /// most 120s of audio for the same reason: that is already enough for
    /// reliable identification, so spending the full O(frame_size^2) chroma
    /// DFT on an arbitrarily long file buys nothing. `sample_rate == 0` or
    /// `max_secs <= 0.0` disables the cap (returns the input unchanged)
    /// rather than dividing by zero or truncating to nothing.
    fn fingerprint_window_len(
        total_samples: usize,
        sample_rate: u32,
        max_secs: f64,
    ) -> (usize, bool) {
        if sample_rate == 0 || max_secs <= 0.0 {
            return (total_samples, false);
        }
        let max_samples = (max_secs * f64::from(sample_rate)) as usize;
        if max_samples == 0 || total_samples <= max_samples {
            (total_samples, false)
        } else {
            (max_samples, true)
        }
    }

    /// Update task progress (also called from `task_transcode.rs`/`task_thumbnail.rs`).
    pub(super) fn update_progress(&self, task_id: TaskId, progress: f64) {
        let mut active_tasks = self.active_tasks.write();
        if let Some(context) = active_tasks.get_mut(&task_id) {
            context.progress = progress;
            tracing::debug!("Task {} progress: {:.1}%", task_id, progress * 100.0);
        }
    }

    /// Get active task count
    #[must_use]
    pub fn active_task_count(&self) -> usize {
        self.active_tasks.read().len()
    }

    /// Get task progress
    #[must_use]
    pub fn get_progress(&self, task_id: TaskId) -> Option<f64> {
        self.active_tasks.read().get(&task_id).map(|c| c.progress)
    }

    /// Cancel a task
    pub async fn cancel_task(&self, task_id: TaskId) -> Result<()> {
        let mut active_tasks = self.active_tasks.write();
        if active_tasks.remove(&task_id).is_some() {
            tracing::info!("Task {} cancelled", task_id);
            Ok(())
        } else {
            Err(FarmError::NotFound(format!("Task {task_id} not found")))
        }
    }
}

impl Default for TaskExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Task specification
#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct TaskSpecification {
    pub(super) task_type: String,
    pub(super) input_path: String,
    pub(super) output_path: String,
    pub(super) parameters: HashMap<String, String>,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_execution_default_payload_is_honest_not_found() {
        // An empty payload falls back to `parse_task_payload`'s placeholder
        // spec (`task_type: "transcode"`, `input_path: "/input/default.mp4"`).
        // That path never exists on a real filesystem, and now that
        // `execute_transcode` is a real transcode (no more
        // sleep-and-return-success simulation), running it must fail
        // honestly instead of fabricating a completed transcode. See
        // `test_execute_transcode_wav_to_flac_round_trip` for the real
        // success path.
        let executor = TaskExecutor::new();
        let task_id = TaskId::new();

        let err = executor
            .execute(task_id, vec![])
            .await
            .expect_err("the default placeholder input path must not exist");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn test_active_task_count() {
        let executor = Arc::new(TaskExecutor::new());
        assert_eq!(executor.active_task_count(), 0);

        let executor_clone = executor.clone();
        let handle =
            tokio::spawn(async move { executor_clone.execute(TaskId::new(), vec![]).await });

        // Wait a bit for task to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Should have 1 active task
        // Note: This is racy, but works for testing
        let _ = handle.await;

        // After completion, should be 0
        assert_eq!(executor.active_task_count(), 0);
    }

    #[tokio::test]
    async fn test_duplicate_task() {
        let executor = TaskExecutor::new();
        let task_id = TaskId::new();

        // Register task as active
        {
            let mut active_tasks = executor.active_tasks.write();
            active_tasks.insert(
                task_id,
                TaskContext {
                    task_id,
                    start_time: Instant::now(),
                    progress: 0.0,
                },
            );
        }

        // Try to execute same task
        let result = executor.execute(task_id, vec![]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_task_progress() {
        let executor = TaskExecutor::new();
        let task_id = TaskId::new();

        // Register task
        {
            let mut active_tasks = executor.active_tasks.write();
            active_tasks.insert(
                task_id,
                TaskContext {
                    task_id,
                    start_time: Instant::now(),
                    progress: 0.0,
                },
            );
        }

        executor.update_progress(task_id, 0.5);
        let progress = executor.get_progress(task_id).unwrap();
        assert!((progress - 0.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_task_cancellation() {
        let executor = TaskExecutor::new();
        let task_id = TaskId::new();

        // Register task
        {
            let mut active_tasks = executor.active_tasks.write();
            active_tasks.insert(
                task_id,
                TaskContext {
                    task_id,
                    start_time: Instant::now(),
                    progress: 0.0,
                },
            );
        }

        executor.cancel_task(task_id).await.unwrap();
        assert_eq!(executor.active_task_count(), 0);
    }

    // -----------------------------------------------------------------
    // Real qc/analysis/fingerprint work (no sleep-simulation)
    // -----------------------------------------------------------------

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_farm_executor_{}_{name}",
            std::process::id()
        ))
    }

    fn wav_bytes(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let data_size = (samples.len() * 2) as u32;
        let byte_rate = sample_rate * u32::from(channels) * 2;
        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&(channels * 2).to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    fn sine_i16(freq: f64, sample_rate: u32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(sample_rate);
                ((t * freq * std::f64::consts::TAU).sin() * 20_000.0) as i16
            })
            .collect()
    }

    fn task_payload(task_type: &str, input_path: &std::path::Path) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "task_type": task_type,
            "input_path": input_path.to_string_lossy(),
            "output_path": "",
            "parameters": {},
        }))
        .expect("serialize task payload")
    }

    #[tokio::test]
    async fn test_execute_qc_wav_produces_real_report() {
        let path = temp_path("qc_real.wav");
        std::fs::write(&path, wav_bytes(&sine_i16(440.0, 44_100, 4_410), 44_100, 1))
            .expect("write wav");

        let executor = TaskExecutor::new();
        let result = executor
            .execute(TaskId::new(), task_payload("qc", &path))
            .await
            .expect("qc task must succeed on a real WAV file");

        assert!(result.success);
        let value: serde_json::Value =
            serde_json::from_slice(&result.output).expect("qc output must be valid JSON");
        assert_eq!(value["kind"], "quality_control");
        assert_eq!(value["container"], "WAV");
        assert!(value["overall_passed"].is_boolean());
        assert!(value["total_checks"].as_u64().unwrap_or(0) > 0);
        assert!(value["results"]
            .as_array()
            .is_some_and(|results| !results.is_empty()));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_qc_unsupported_format_is_honest_err() {
        let path = temp_path("qc_unsupported.bin");
        std::fs::write(&path, b"this is not any known media container").expect("write bin");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(TaskId::new(), task_payload("qc", &path))
            .await
            .expect_err("qc on an unrecognised container must fail honestly");
        assert!(err.to_string().contains("synthesise"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_qc_missing_file_is_not_found() {
        let path = temp_path("qc_missing.wav");
        let _ = std::fs::remove_file(&path);

        let executor = TaskExecutor::new();
        let err = executor
            .execute(TaskId::new(), task_payload("qc", &path))
            .await
            .expect_err("qc on a missing file must fail");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn test_execute_analysis_wav_produces_real_summary() {
        let path = temp_path("analysis_real.wav");
        std::fs::write(&path, wav_bytes(&sine_i16(440.0, 44_100, 8_820), 44_100, 1))
            .expect("write wav");

        let executor = TaskExecutor::new();
        let result = executor
            .execute(TaskId::new(), task_payload("analysis", &path))
            .await
            .expect("analysis task must succeed on a real WAV file");

        assert!(result.success);
        let value: serde_json::Value =
            serde_json::from_slice(&result.output).expect("analysis output must be valid JSON");
        assert_eq!(value["kind"], "analysis");
        assert_eq!(value["sample_count"], 8_820);
        let centroid = value["spectral"]["centroid_hz"]
            .as_f64()
            .expect("spectral centroid must be present");
        assert!(centroid.is_finite() && centroid > 0.0);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_analysis_unsupported_format_is_honest_err() {
        let path = temp_path("analysis_unsupported.bin");
        std::fs::write(&path, b"YUV4MPEG2 W16 H16 F25:1 Ip A1:1 C420jpeg\n").expect("write");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(TaskId::new(), task_payload("analysis", &path))
            .await
            .expect_err("analysis on a non-WAV container must fail honestly");
        assert!(err.to_string().contains("RIFF/WAVE"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_fingerprint_distinct_inputs_give_distinct_fingerprints() {
        // One frame's worth of samples at the encoder's real (production)
        // frame size (4096 — see `AcoustidEncoder::compute`). A higher
        // sample rate keeps the O(frame_size^2) chroma DFT's per-frame
        // in-range bin count (and thus test runtime) small while still
        // separating 440 Hz and 523.25 Hz into different pitch classes —
        // the same pair `oximedia-mir`'s own tests prove distinguishable.
        let path_a = temp_path("fp_a.wav");
        let path_b = temp_path("fp_b.wav");
        std::fs::write(
            &path_a,
            wav_bytes(&sine_i16(440.0, 48_000, 4_200), 48_000, 1),
        )
        .expect("write wav a");
        std::fs::write(
            &path_b,
            wav_bytes(&sine_i16(523.25, 48_000, 4_200), 48_000, 1),
        )
        .expect("write wav b");

        let executor = TaskExecutor::new();

        let result_a = executor
            .execute(TaskId::new(), task_payload("fingerprint", &path_a))
            .await
            .expect("fingerprint task must succeed on a real WAV file");
        let result_b = executor
            .execute(TaskId::new(), task_payload("fingerprint", &path_b))
            .await
            .expect("fingerprint task must succeed on a real WAV file");

        assert!(result_a.success && result_b.success);
        let value_a: serde_json::Value =
            serde_json::from_slice(&result_a.output).expect("fingerprint output a JSON");
        let value_b: serde_json::Value =
            serde_json::from_slice(&result_b.output).expect("fingerprint output b JSON");

        assert_eq!(value_a["kind"], "fingerprint");
        let fp_a = value_a["fingerprint"]
            .as_array()
            .expect("fingerprint array a")
            .clone();
        let fp_b = value_b["fingerprint"]
            .as_array()
            .expect("fingerprint array b")
            .clone();
        assert!(!fp_a.is_empty(), "fingerprint must be non-empty");
        assert!(!fp_b.is_empty(), "fingerprint must be non-empty");
        assert_ne!(
            fp_a, fp_b,
            "different audio content must produce different fingerprints"
        );

        // Determinism: re-running on file A must reproduce the identical
        // fingerprint — otherwise `assert_ne!` above would be one FNV
        // collision away from proving nothing.
        let result_a_again = executor
            .execute(TaskId::new(), task_payload("fingerprint", &path_a))
            .await
            .expect("fingerprint task must succeed on a real WAV file");
        let value_a_again: serde_json::Value =
            serde_json::from_slice(&result_a_again.output).expect("fingerprint output a2 JSON");
        assert_eq!(
            value_a_again["fingerprint"], value_a["fingerprint"],
            "the same audio must produce the same fingerprint every time"
        );

        let _ = std::fs::remove_file(&path_a);
        let _ = std::fs::remove_file(&path_b);
    }

    #[test]
    fn test_fingerprint_window_len_caps_long_audio() {
        // Below the cap: passed through unchanged.
        let (len, truncated) = TaskExecutor::fingerprint_window_len(1_000, 8_000, 120.0);
        assert_eq!(len, 1_000);
        assert!(!truncated);

        // Above the cap: clamped to exactly `max_secs` worth of samples.
        let (len, truncated) = TaskExecutor::fingerprint_window_len(2_000_000, 8_000, 120.0);
        assert_eq!(len, 8_000 * 120);
        assert!(truncated);

        // Degenerate inputs disable the cap rather than dividing by zero or
        // truncating everything away.
        let (len, truncated) = TaskExecutor::fingerprint_window_len(1_000, 0, 120.0);
        assert_eq!(len, 1_000);
        assert!(!truncated);
        let (len, truncated) = TaskExecutor::fingerprint_window_len(1_000, 8_000, 0.0);
        assert_eq!(len, 1_000);
        assert!(!truncated);
    }

    #[tokio::test]
    async fn test_execute_fingerprint_truncates_to_analysis_window() {
        // 0.2s @ 48kHz mono, capped to a 0.1s analysis window (a tiny cap
        // purely so this test stays fast — see `MAX_FINGERPRINT_SECS` for
        // the real 120s production value). Exercises the actual truncation
        // path end-to-end, not just the pure `fingerprint_window_len` math.
        let path = temp_path("fp_capped.wav");
        std::fs::write(&path, wav_bytes(&sine_i16(440.0, 48_000, 9_600), 48_000, 1))
            .expect("write wav");

        let executor = TaskExecutor::new();
        let spec = TaskSpecification {
            task_type: "fingerprint".to_string(),
            input_path: path.to_string_lossy().to_string(),
            output_path: String::new(),
            parameters: HashMap::new(),
        };
        let output = executor
            .execute_fingerprint_capped(TaskId::new(), &spec, 0.1)
            .await
            .expect("capped fingerprint must succeed");
        let value: serde_json::Value = serde_json::from_slice(&output).expect("json output");

        assert_eq!(value["truncated_to_analysis_window"], true);
        let input_secs = value["input_duration_secs"]
            .as_f64()
            .expect("input_duration_secs");
        let fingerprinted_secs = value["fingerprinted_secs"]
            .as_f64()
            .expect("fingerprinted_secs");
        assert!((input_secs - 0.2).abs() < 1e-6, "{input_secs}");
        assert!(
            fingerprinted_secs < input_secs,
            "fingerprinted_secs ({fingerprinted_secs}) must be less than the full \
             input_duration_secs ({input_secs}) once truncated"
        );
        assert!(
            (fingerprinted_secs - 0.1).abs() < 1e-6,
            "{fingerprinted_secs}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    #[ignore = "perf measurement scratch — not part of the CI gate. Result (2026-08): \
                10s of 48kHz mono audio, uncapped, took 2.68s release / 28.35s debug — \
                the measurement that set MAX_FINGERPRINT_SECS and the blocking dispatch."]
    async fn measure_fingerprint_10s_48k() {
        let path = temp_path("perf_10s.wav");
        let samples = sine_i16(440.0, 48_000, 480_000);
        std::fs::write(&path, wav_bytes(&samples, 48_000, 1)).expect("write wav");

        let executor = TaskExecutor::new();
        let start = std::time::Instant::now();
        let result = executor
            .execute(TaskId::new(), task_payload("fingerprint", &path))
            .await;
        let elapsed = start.elapsed();
        eprintln!(
            "10s @ 48kHz mono fingerprint: {elapsed:?}, ok={}",
            result.is_ok()
        );
        if let Ok(r) = &result {
            let v: serde_json::Value = serde_json::from_slice(&r.output).expect("json");
            eprintln!("hash_count={}", v["hash_count"]);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_fingerprint_unsupported_format_is_honest_err() {
        let path = temp_path("fp_unsupported.bin");
        std::fs::write(&path, b"not a wav file at all").expect("write bin");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(TaskId::new(), task_payload("fingerprint", &path))
            .await
            .expect_err("fingerprint on a non-WAV container must fail honestly");
        assert!(err.to_string().contains("RIFF/WAVE"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_resolve_qc_preset_defaults_and_rejects_unknown() {
        let empty = HashMap::new();
        assert!(matches!(
            TaskExecutor::resolve_qc_preset(&empty),
            Ok(oximedia_qc::QcPreset::Basic)
        ));

        let mut params = HashMap::new();
        params.insert("qc_profile".to_string(), "Broadcast".to_string());
        assert!(matches!(
            TaskExecutor::resolve_qc_preset(&params),
            Ok(oximedia_qc::QcPreset::Broadcast)
        ));

        let mut bad = HashMap::new();
        bad.insert("qc_profile".to_string(), "ultra-broadcast".to_string());
        let err = TaskExecutor::resolve_qc_preset(&bad).expect_err("unknown profile must error");
        assert!(err.to_string().contains("ultra_broadcast"), "{err}");
    }
}
