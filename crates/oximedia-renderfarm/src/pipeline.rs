// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Rendering pipeline management (pre-render, render, post-render).

use crate::asset_fetch::AssetSource;
use crate::error::{Error, Result};
use crate::job::{Job, JobId};
use crate::worker::WorkerId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Pipeline stage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineStage {
    /// Pre-render stage (validation, setup)
    PreRender,
    /// Render stage (actual rendering)
    Render,
    /// Post-render stage (verification, assembly)
    PostRender,
}

impl std::fmt::Display for PipelineStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreRender => write!(f, "PreRender"),
            Self::Render => write!(f, "Render"),
            Self::PostRender => write!(f, "PostRender"),
        }
    }
}

/// Pipeline task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTask {
    /// Task ID
    pub id: String,
    /// Job ID
    pub job_id: JobId,
    /// Stage
    pub stage: PipelineStage,
    /// Status
    pub status: TaskStatus,
    /// Started at
    pub started_at: Option<DateTime<Utc>>,
    /// Completed at
    pub completed_at: Option<DateTime<Utc>>,
    /// Error message
    pub error: Option<String>,
}

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Pending
    Pending,
    /// Running
    Running,
    /// Completed
    Completed,
    /// Failed
    Failed,
}

/// Pre-render result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreRenderResult {
    /// Assets verified
    pub assets_verified: bool,
    /// Dependencies resolved
    pub dependencies_resolved: bool,
    /// Estimated frames
    pub estimated_frames: u32,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Estimated time (seconds)
    pub estimated_time: f64,
    /// Issues found
    pub issues: Vec<String>,
}

/// Render result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderResult {
    /// Frame number
    pub frame: u32,
    /// Output path
    pub output_path: PathBuf,
    /// Render time (seconds)
    pub render_time: f64,
    /// Worker ID
    pub worker_id: WorkerId,
    /// Success
    pub success: bool,
    /// Error message
    pub error: Option<String>,
    /// SHA-256 checksum of the output file's bytes (hex-encoded), as
    /// recorded by whatever produced this result right after writing the
    /// file (see [`crate::checksum::sha256_file`]). `None` when no
    /// checksum was recorded; post-render verification then falls back to
    /// an existence/non-emptiness check only, since there is no baseline to
    /// detect corruption against.
    pub checksum: Option<String>,
}

/// Post-render result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostRenderResult {
    /// All frames verified
    pub all_frames_verified: bool,
    /// Output assembled
    pub output_assembled: bool,
    /// Final output path
    pub final_output_path: Option<PathBuf>,
    /// Quality metrics
    pub quality_metrics: HashMap<String, f64>,
}

/// Pipeline executor
pub struct Pipeline {
    tasks: HashMap<JobId, Vec<PipelineTask>>,
    pre_render_results: HashMap<JobId, PreRenderResult>,
    render_results: HashMap<JobId, Vec<RenderResult>>,
    post_render_results: HashMap<JobId, PostRenderResult>,
    /// Configured asset-source locations searched (in order) by
    /// [`Self::resolve_dependencies`] for a dependency that is missing at
    /// its declared path. Empty by default, which preserves the original
    /// existence-only resolution behavior.
    asset_sources: Vec<AssetSource>,
}

impl Pipeline {
    /// Create a new pipeline
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            pre_render_results: HashMap::new(),
            render_results: HashMap::new(),
            post_render_results: HashMap::new(),
            asset_sources: Vec::new(),
        }
    }

    /// Registers a configured asset-source location that
    /// `Self::resolve_dependencies` will search (in the order added) for a
    /// missing dependency before declaring it unresolved. Accepts a local
    /// directory path, a `file://` URI, or an `http(s)://` base URL -- see
    /// `crate::asset_fetch::AssetSource::parse`.
    pub fn add_asset_source(&mut self, source: impl AsRef<str>) {
        self.asset_sources.push(AssetSource::parse(source.as_ref()));
    }

    /// Execute pre-render stage
    pub async fn execute_pre_render(&mut self, job: &Job) -> Result<PreRenderResult> {
        let task = PipelineTask {
            id: format!("{}-prerender", job.id),
            job_id: job.id,
            stage: PipelineStage::PreRender,
            status: TaskStatus::Running,
            started_at: Some(Utc::now()),
            completed_at: None,
            error: None,
        };

        self.tasks.entry(job.id).or_default().push(task);

        // Dependency resolution runs *before* asset verification: a missing
        // dependency that a configured asset source can supply (see
        // `Self::add_asset_source`) must actually be copied/downloaded into
        // place first, so `verify_assets`'s existence check below sees the
        // post-resolution state. Checking these in the other order would
        // make `add_asset_source` inert through this entry point -- it
        // would resolve the dependency one line too late to matter, and
        // `verify_assets` would still honestly (but misleadingly) report
        // "could not be verified" for a dependency that was, in fact, just
        // resolved.
        let dependencies_resolved = self.resolve_dependencies(job).await?;

        // Asset verification (now sees any dependency resolve_dependencies
        // just materialized).
        let assets_verified = self.verify_assets(job).await?;

        // Estimate resources
        let (estimated_frames, estimated_cost, estimated_time) = self.estimate_resources(job);

        // Collect issues
        let mut issues = Vec::new();
        if !assets_verified {
            issues.push("Some assets could not be verified".to_string());
        }
        if !dependencies_resolved {
            issues.push("Some dependencies could not be resolved".to_string());
        }

        let result = PreRenderResult {
            assets_verified,
            dependencies_resolved,
            estimated_frames,
            estimated_cost,
            estimated_time,
            issues,
        };

        // Update task
        if let Some(task) = self
            .tasks
            .get_mut(&job.id)
            .and_then(|tasks| tasks.last_mut())
        {
            task.status = if result.issues.is_empty() {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            };
            task.completed_at = Some(Utc::now());
            if !result.issues.is_empty() {
                task.error = Some(result.issues.join(", "));
            }
        }

        self.pre_render_results.insert(job.id, result.clone());

        Ok(result)
    }

    /// Record render result
    pub fn record_render_result(&mut self, job_id: JobId, result: RenderResult) {
        self.render_results.entry(job_id).or_default().push(result);
    }

    /// Marks the most recently pushed pipeline task for `job_id` as
    /// `Failed` with `error`'s message, and stamps `completed_at`. Shared by
    /// [`Self::execute_post_render`]'s three post-render stages so a real
    /// failure is always recorded on the task instead of leaving it stuck
    /// at `Running`.
    fn fail_last_task(&mut self, job_id: JobId, error: &Error) {
        if let Some(task) = self
            .tasks
            .get_mut(&job_id)
            .and_then(|tasks| tasks.last_mut())
        {
            task.status = TaskStatus::Failed;
            task.completed_at = Some(Utc::now());
            task.error = Some(error.to_string());
        }
    }

    /// Execute post-render stage.
    ///
    /// Runs real frame verification (including per-frame checksum
    /// corruption detection), real output assembly (MJPEG-in-AVI or Y4M
    /// muxing via `oximedia-container`), and real quality metrics
    /// (PSNR/SSIM against a configured reference, or no-reference
    /// blockiness/blur otherwise). Any stage that fails records the real
    /// error on the pipeline task and marks it `Failed` before propagating
    /// `Err` -- the task is never left dangling at `Running`, and this never
    /// fabricates a completed [`PostRenderResult`].
    pub async fn execute_post_render(&mut self, job: &Job) -> Result<PostRenderResult> {
        let task = PipelineTask {
            id: format!("{}-postrender", job.id),
            job_id: job.id,
            stage: PipelineStage::PostRender,
            status: TaskStatus::Running,
            started_at: Some(Utc::now()),
            completed_at: None,
            error: None,
        };

        self.tasks.entry(job.id).or_default().push(task);

        // Verify all frames (real check against recorded render results,
        // including checksum corruption detection). A corruption or
        // verification `Err` must record the task as `Failed` before
        // propagating, not leave it stuck at `Running`.
        let all_frames_verified = match self.verify_all_frames(job).await {
            Ok(v) => v,
            Err(e) => {
                self.fail_last_task(job.id, &e);
                return Err(e);
            }
        };

        // Assemble output (real MJPEG-in-AVI or Y4M muxing) — record the
        // failure honestly on the task before propagating, instead of
        // leaving it stuck at `Running`.
        let (output_assembled, final_output_path) = match self.assemble_output(job).await {
            Ok(v) => v,
            Err(e) => {
                self.fail_last_task(job.id, &e);
                return Err(e);
            }
        };

        // Calculate quality metrics (real PSNR/SSIM or blockiness/blur) —
        // same honest-failure handling as the two stages above.
        let quality_metrics = match self.calculate_quality_metrics(job).await {
            Ok(v) => v,
            Err(e) => {
                self.fail_last_task(job.id, &e);
                return Err(e);
            }
        };

        let result = PostRenderResult {
            all_frames_verified,
            output_assembled,
            final_output_path,
            quality_metrics,
        };

        // Update task
        if let Some(task) = self
            .tasks
            .get_mut(&job.id)
            .and_then(|tasks| tasks.last_mut())
        {
            task.status = if all_frames_verified && output_assembled {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            };
            task.completed_at = Some(Utc::now());
        }

        self.post_render_results.insert(job.id, result.clone());

        Ok(result)
    }

    /// Verify assets
    async fn verify_assets(&self, job: &Job) -> Result<bool> {
        // Check if project file exists
        if !job.submission.project_file.exists() {
            return Ok(false);
        }

        // Check all dependencies
        for dep in &job.submission.dependencies {
            if !dep.exists() {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Resolve dependencies.
    ///
    /// Real check: every declared dependency asset must exist on disk (a job
    /// with no declared dependencies is vacuously resolved — there is
    /// nothing to resolve). A missing dependency is not immediately given
    /// up on: it is searched for, in order, across any configured
    /// [`AssetSource`]s (see [`Self::add_asset_source`]) and, if found,
    /// copied or downloaded into place at its declared path — real
    /// local-path and `file://` copies, and real `http(s)://` downloads via
    /// `reqwest`. A dependency (or configured source) naming a scheme with
    /// no available transport (e.g. `s3://`) fails honestly instead of
    /// being silently reported as just "missing" — see
    /// [`crate::asset_fetch::resolve_missing_dependency`].
    ///
    /// An earlier revision returned `!dependencies.is_empty()` — a
    /// fabricated signal derived from list length rather than any real
    /// resolution check (and backwards: it reported jobs with *no*
    /// dependencies as unresolved).
    async fn resolve_dependencies(&self, job: &Job) -> Result<bool> {
        for dep in &job.submission.dependencies {
            if dep.exists() {
                continue;
            }
            if !crate::asset_fetch::resolve_missing_dependency(dep, &self.asset_sources).await? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Estimate resources
    fn estimate_resources(&self, job: &Job) -> (u32, f64, f64) {
        // Get frame count
        let frame_count = match &job.submission.job_type {
            crate::job::JobType::ImageSequence {
                start_frame,
                end_frame,
            } => end_frame - start_frame + 1,
            crate::job::JobType::VideoRender { .. } => 100,
            _ => 1,
        };

        // Estimate cost and time
        let estimated_cost = f64::from(frame_count) * 0.01;
        let estimated_time = f64::from(frame_count) * 10.0; // 10 seconds per frame

        (frame_count, estimated_cost, estimated_time)
    }

    /// Verify all frames.
    ///
    /// Real check: every recorded [`RenderResult`] for the job must report
    /// `success`, and its `output_path` must exist on disk and be
    /// non-empty. If no render results have been recorded yet (or fewer
    /// were recorded than the pre-render stage estimated), verification
    /// honestly reports `false` rather than the previous hardcoded `true`.
    ///
    /// When a `RenderResult` also carries a recorded
    /// [`RenderResult::checksum`], the output file's *current* SHA-256 is
    /// recomputed from disk and compared against it: a mismatch means the
    /// bytes changed since they were recorded (truncation, bit rot, a
    /// concurrent overwrite, a bad transfer, ...) and is reported as a real
    /// corruption `Err` naming the job and frame — not a bare `false`,
    /// because the caller ([`Self::execute_post_render`]) needs the real
    /// reason to record an honest task error. A frame with no recorded
    /// checksum has no baseline to detect corruption against, so it only
    /// gets the existence/non-emptiness check.
    async fn verify_all_frames(&self, job: &Job) -> Result<bool> {
        let Some(results) = self.render_results.get(&job.id) else {
            return Ok(false);
        };
        if results.is_empty() {
            return Ok(false);
        }

        if let Some(pre) = self.pre_render_results.get(&job.id) {
            if (results.len() as u32) < pre.estimated_frames {
                return Ok(false);
            }
        }

        for result in results {
            if !result.success {
                return Ok(false);
            }
            match std::fs::metadata(&result.output_path) {
                Ok(meta) if meta.len() > 0 => {}
                _ => return Ok(false),
            }

            if let Some(expected) = &result.checksum {
                let actual = crate::checksum::sha256_file(&result.output_path)?;
                if &actual != expected {
                    return Err(Error::VerificationFailed(format!(
                        "job {} frame {}: checksum mismatch for {} (expected {expected}, got \
                         {actual}) — output file is corrupt",
                        job.id,
                        result.frame,
                        result.output_path.display()
                    )));
                }
            }
        }

        Ok(true)
    }

    /// Assemble output.
    ///
    /// Real muxing via `oximedia-container`: rendered JPEG frames are muxed
    /// byte-for-byte as Motion JPEG into an AVI container
    /// ([`oximedia_container::mux::avi::AviMjpegWriter`]); any other
    /// decodable frame format (PNG, WebP, ...) is decoded, normalized to
    /// YUV420p, and muxed as raw frames into a Y4M stream. See
    /// [`crate::output_assembly`] for the full routing rules and their
    /// honest limits (frame-rate default, dimension/format consistency
    /// requirements). An earlier revision fabricated a hardcoded
    /// `/output/{job_id}.mp4` path and reported `output_assembled: true`
    /// without writing anything to disk — this now does the real
    /// assembly or fails honestly, never both a `false`/`None` result and
    /// an `Ok`.
    async fn assemble_output(&self, job: &Job) -> Result<(bool, Option<PathBuf>)> {
        let frame_paths = self.successful_frame_paths(job.id);
        let output_path = crate::output_assembly::assemble(job, &frame_paths)?;
        Ok((true, Some(output_path)))
    }

    /// Calculate quality metrics.
    ///
    /// Real computation via `oximedia-quality`: when the job submission
    /// names a reference image (`metadata["quality_reference"]`),
    /// full-reference PSNR/SSIM is computed against it; otherwise
    /// no-reference blockiness/blur is computed directly from the rendered
    /// frames. See [`crate::quality_metrics`] for the exact algorithm
    /// selection and honest failure conditions (no frames, undecodable
    /// frames, dimension mismatches). An earlier revision fabricated
    /// `psnr: 42.0, ssim: 0.95` for every job regardless of content — no
    /// score returned here is ever hardcoded.
    async fn calculate_quality_metrics(&self, job: &Job) -> Result<HashMap<String, f64>> {
        let frame_paths = self.successful_frame_paths(job.id);
        crate::quality_metrics::calculate(job, &frame_paths)
    }

    /// Collects the output paths of every `success` [`RenderResult`]
    /// recorded for `job_id`, sorted by frame number. Shared by
    /// [`Self::assemble_output`] and [`Self::calculate_quality_metrics`],
    /// which both need the same real, ordered frame list.
    fn successful_frame_paths(&self, job_id: JobId) -> Vec<PathBuf> {
        let mut successful: Vec<&RenderResult> = self
            .render_results
            .get(&job_id)
            .map(|results| results.iter().filter(|r| r.success).collect())
            .unwrap_or_default();
        successful.sort_by_key(|r| r.frame);
        successful
            .into_iter()
            .map(|r| r.output_path.clone())
            .collect()
    }

    /// Get pre-render result
    #[must_use]
    pub fn get_pre_render_result(&self, job_id: JobId) -> Option<&PreRenderResult> {
        self.pre_render_results.get(&job_id)
    }

    /// Get render results
    #[must_use]
    pub fn get_render_results(&self, job_id: JobId) -> Vec<&RenderResult> {
        self.render_results
            .get(&job_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Get post-render result
    #[must_use]
    pub fn get_post_render_result(&self, job_id: JobId) -> Option<&PostRenderResult> {
        self.post_render_results.get(&job_id)
    }

    /// Get pipeline tasks for job
    #[must_use]
    pub fn get_tasks(&self, job_id: JobId) -> Vec<&PipelineTask> {
        self.tasks
            .get(&job_id)
            .map_or_else(Vec::new, |tasks| tasks.iter().collect())
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{JobSubmission, Priority};

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia-renderfarm-pipeline-{name}"))
    }

    #[tokio::test]
    async fn test_pipeline_creation() {
        let pipeline = Pipeline::new();
        assert_eq!(pipeline.tasks.len(), 0);
    }

    #[tokio::test]
    async fn test_pre_render_execution() -> Result<()> {
        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("test.blend"))
            .frame_range(1, 10)
            .priority(Priority::Normal)
            .build()?;

        let job = Job::new(submission);

        let result = pipeline.execute_pre_render(&job).await?;
        assert!(result.estimated_frames > 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_post_render_execution_fails_honestly_with_no_recorded_frames() -> Result<()> {
        // Output assembly and quality metrics are real now (MJPEG-in-AVI/
        // Y4M muxing, PSNR/SSIM/blockiness/blur) but this job never had any
        // render results recorded, so there is nothing to assemble or
        // measure. execute_post_render must honestly report that via `Err`
        // instead of fabricating a finished output — see
        // `test_execute_post_render_real_end_to_end_success` for the real
        // success path with actual recorded frames.
        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("test.blend"))
            .frame_range(1, 10)
            .build()?;

        let job = Job::new(submission);

        let result = pipeline.execute_post_render(&job).await;
        assert!(
            result.is_err(),
            "post-render must not fabricate a completed assembly"
        );

        // The task must be recorded as Failed with the real error message,
        // not left dangling at `Running` forever.
        let tasks = pipeline.get_tasks(job.id);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Failed);
        assert!(tasks[0].error.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_dependencies_real_check() -> Result<()> {
        let pipeline = Pipeline::new();

        // No dependencies declared: vacuously resolved (nothing to
        // resolve). The old `!dependencies.is_empty()` fabrication would
        // have reported this case as `false`.
        let submission_empty = JobSubmission::builder()
            .project_file(tmp_path("resolve-deps-empty.blend"))
            .frame_range(1, 5)
            .build()?;
        let job_empty = Job::new(submission_empty);
        assert!(pipeline.resolve_dependencies(&job_empty).await?);

        // A declared dependency that does not exist on disk: not resolved.
        let missing_dep = tmp_path("resolve-deps-missing-dep.bin");
        let _ = std::fs::remove_file(&missing_dep);
        let submission_missing = JobSubmission::builder()
            .project_file(tmp_path("resolve-deps-missing.blend"))
            .frame_range(1, 5)
            .dependency(missing_dep)
            .build()?;
        let job_missing = Job::new(submission_missing);
        assert!(!pipeline.resolve_dependencies(&job_missing).await?);

        // A declared dependency that does exist on disk: resolved.
        let present_dep = tmp_path("resolve-deps-present-dep.bin");
        std::fs::write(&present_dep, b"asset bytes")?;
        let submission_present = JobSubmission::builder()
            .project_file(tmp_path("resolve-deps-present.blend"))
            .frame_range(1, 5)
            .dependency(present_dep.clone())
            .build()?;
        let job_present = Job::new(submission_present);
        assert!(pipeline.resolve_dependencies(&job_present).await?);

        std::fs::remove_file(&present_dep).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_verify_all_frames_real_check() -> Result<()> {
        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("verify-frames.blend"))
            .frame_range(1, 2)
            .build()?;
        let job = Job::new(submission);

        // No render results recorded yet: honestly not verified (the old
        // code hardcoded `true` here regardless).
        assert!(!pipeline.verify_all_frames(&job).await?);

        // A "successful" result pointing at a file that does not actually
        // exist must still fail real verification.
        pipeline.record_render_result(
            job.id,
            RenderResult {
                frame: 1,
                output_path: tmp_path("verify-frames-missing.png"),
                render_time: 1.0,
                worker_id: WorkerId::new(),
                success: true,
                error: None,
                checksum: None,
            },
        );
        assert!(!pipeline.verify_all_frames(&job).await?);

        // A real, non-empty frame file on disk: now it verifies.
        let real_frame = tmp_path("verify-frames-real.png");
        std::fs::write(&real_frame, b"not really a png but non-empty")?;
        let mut pipeline2 = Pipeline::new();
        pipeline2.record_render_result(
            job.id,
            RenderResult {
                frame: 1,
                output_path: real_frame.clone(),
                render_time: 1.0,
                worker_id: WorkerId::new(),
                success: true,
                error: None,
                checksum: None,
            },
        );
        assert!(pipeline2.verify_all_frames(&job).await?);

        std::fs::remove_file(&real_frame).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_verify_all_frames_detects_checksum_corruption() -> Result<()> {
        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("checksum.blend"))
            .frame_range(1, 1)
            .build()?;
        let job = Job::new(submission);

        let frame_path = tmp_path("checksum-frame.bin");
        let original_bytes: &[u8] = b"this is the real rendered frame content";
        std::fs::write(&frame_path, original_bytes)?;
        let recorded_checksum = crate::checksum::sha256_hex(original_bytes);

        pipeline.record_render_result(
            job.id,
            RenderResult {
                frame: 1,
                output_path: frame_path.clone(),
                render_time: 1.0,
                worker_id: WorkerId::new(),
                success: true,
                error: None,
                checksum: Some(recorded_checksum.clone()),
            },
        );

        // Bytes on disk still match the recorded checksum: verifies clean.
        assert!(pipeline.verify_all_frames(&job).await?);

        // Simulate corruption: the file's bytes change after the checksum
        // was recorded (bit rot, truncation, a bad transfer, ...).
        std::fs::write(&frame_path, b"corrupted content, different bytes entirely")?;

        let result = pipeline.verify_all_frames(&job).await;
        assert!(
            result.is_err(),
            "a checksum mismatch must be a real corruption Err, not a bare false"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains(&job.id.to_string()) && message.contains("frame 1"),
            "corruption error should name the job and frame: {message}"
        );
        assert!(
            message.contains(&recorded_checksum[..8]),
            "corruption error should include the expected checksum: {message}"
        );

        std::fs::remove_file(&frame_path).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_post_render_records_failed_task_on_checksum_corruption() -> Result<()> {
        // Regression test: verify_all_frames returning `Err` (instead of
        // `Ok(false)`) must still result in the pipeline task being marked
        // `Failed`, not left dangling at `Running` — the same honesty
        // contract execute_post_render already upheld for assemble_output.
        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("checksum-e2e.blend"))
            .frame_range(1, 1)
            .build()?;
        let job = Job::new(submission);

        let frame_path = tmp_path("checksum-e2e-frame.bin");
        std::fs::write(&frame_path, b"tampered bytes")?;

        pipeline.record_render_result(
            job.id,
            RenderResult {
                frame: 1,
                output_path: frame_path.clone(),
                render_time: 1.0,
                worker_id: WorkerId::new(),
                success: true,
                error: None,
                // Deliberately wrong checksum, as if it were recorded
                // against the frame's original (pre-corruption) bytes.
                checksum: Some(crate::checksum::sha256_hex(b"original bytes")),
            },
        );

        let result = pipeline.execute_post_render(&job).await;
        assert!(result.is_err(), "corruption must fail post-render");

        let tasks = pipeline.get_tasks(job.id);
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].status,
            TaskStatus::Failed,
            "a corruption Err from verify_all_frames must still mark the task Failed, not \
             leave it stuck at Running"
        );
        assert!(tasks[0].error.is_some());

        std::fs::remove_file(&frame_path).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_assemble_output_is_honest_err() -> Result<()> {
        let pipeline = Pipeline::new();
        let submission = JobSubmission::builder()
            .project_file(tmp_path("assemble.blend"))
            .frame_range(1, 5)
            .build()?;
        let job = Job::new(submission);

        let result = pipeline.assemble_output(&job).await;
        assert!(
            result.is_err(),
            "assemble_output must not fabricate a finished output path"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_calculate_quality_metrics_is_honest_err() -> Result<()> {
        let pipeline = Pipeline::new();
        let submission = JobSubmission::builder()
            .project_file(tmp_path("quality.blend"))
            .frame_range(1, 5)
            .build()?;
        let job = Job::new(submission);

        let result = pipeline.calculate_quality_metrics(&job).await;
        assert!(
            result.is_err(),
            "calculate_quality_metrics must not fabricate hardcoded psnr/ssim values"
        );

        Ok(())
    }

    #[test]
    fn test_pipeline_stage_display() {
        assert_eq!(PipelineStage::PreRender.to_string(), "PreRender");
        assert_eq!(PipelineStage::Render.to_string(), "Render");
        assert_eq!(PipelineStage::PostRender.to_string(), "PostRender");
    }

    #[tokio::test]
    async fn test_get_tasks() -> Result<()> {
        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("test.blend"))
            .frame_range(1, 10)
            .build()?;

        let job = Job::new(submission);
        let job_id = job.id;

        pipeline.execute_pre_render(&job).await?;

        let tasks = pipeline.get_tasks(job_id);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].stage, PipelineStage::PreRender);

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_post_render_real_end_to_end_success() -> Result<()> {
        // Real frames all the way through: verification (with checksums),
        // assembly (PNG frames -> real Y4M mux), and quality metrics
        // (no reference configured -> real no-reference blockiness/blur).
        // Nothing here is fabricated; every field asserted below comes from
        // actually decoding/muxing/measuring the frames written to disk.
        use oximedia_codec::frame::{Plane, VideoFrame as CodecVideoFrame};
        use oximedia_codec::image::{EncoderConfig, ImageEncoder};
        use oximedia_core::PixelFormat;

        let mut pipeline = Pipeline::new();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("e2e-success.blend"))
            .frame_range(1, 3)
            .build()?;
        let job = Job::new(submission);

        let width = 32u32;
        let height = 32u32;
        let encoder = ImageEncoder::new(EncoderConfig::png());

        let mut frame_paths = Vec::new();
        for frame_num in 1..=3u32 {
            let mut rgb = Vec::with_capacity((width * height * 3) as usize);
            for y in 0..height {
                for x in 0..width {
                    rgb.push(((x + frame_num) * 4) as u8);
                    rgb.push((y * 4) as u8);
                    rgb.push(100);
                }
            }
            let mut codec_frame = CodecVideoFrame::new(PixelFormat::Rgb24, width, height);
            codec_frame.planes = vec![Plane::with_dimensions(
                rgb,
                (width * 3) as usize,
                width,
                height,
            )];
            let png_bytes = encoder
                .encode(&codec_frame)
                .map_err(|e| Error::Other(format!("test PNG encode failed: {e}")))?;

            let path = tmp_path(&format!("e2e-success-frame-{frame_num}.png"));
            std::fs::write(&path, &png_bytes)?;
            let checksum = crate::checksum::sha256_hex(&png_bytes);

            pipeline.record_render_result(
                job.id,
                RenderResult {
                    frame: frame_num,
                    output_path: path.clone(),
                    render_time: 1.0,
                    worker_id: WorkerId::new(),
                    success: true,
                    error: None,
                    checksum: Some(checksum),
                },
            );
            frame_paths.push(path);
        }

        let result = pipeline.execute_post_render(&job).await?;
        assert!(result.all_frames_verified);
        assert!(result.output_assembled);

        let output_path = result
            .final_output_path
            .clone()
            .ok_or_else(|| Error::Other("expected a final output path".to_string()))?;
        assert!(output_path.exists());
        assert!(std::fs::metadata(&output_path)?.len() > 0);

        // No `quality_reference` metadata was set: no-reference metrics.
        assert!(result.quality_metrics.contains_key("blockiness"));
        assert!(result.quality_metrics.contains_key("blur"));
        for (name, value) in &result.quality_metrics {
            assert!(
                value.is_finite(),
                "{name} should be a real finite number, got {value}"
            );
        }

        let tasks = pipeline.get_tasks(job.id);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed);

        for path in &frame_paths {
            std::fs::remove_file(path).ok();
        }
        std::fs::remove_file(&output_path).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_add_asset_source_resolves_missing_dependency_from_local_source() -> Result<()> {
        let source_dir = tmp_path("pipeline-asset-source-dir");
        std::fs::create_dir_all(&source_dir)?;
        std::fs::write(source_dir.join("shared_texture.bin"), b"shared asset bytes")?;

        let mut pipeline = Pipeline::new();
        pipeline.add_asset_source(source_dir.to_string_lossy());

        // The declared dependency path lives under a *different* temp
        // directory than the configured source; resolution must copy the
        // source's file (matched by file name) into place here.
        let declared = tmp_path("pipeline-asset-source-dest").join("shared_texture.bin");
        std::fs::remove_file(&declared).ok();

        let submission = JobSubmission::builder()
            .project_file(tmp_path("asset-source.blend"))
            .frame_range(1, 1)
            .dependency(declared.clone())
            .build()?;
        let job = Job::new(submission);

        let resolved = pipeline.resolve_dependencies(&job).await?;
        assert!(
            resolved,
            "dependency should be resolved from the configured asset source"
        );
        assert!(declared.exists());
        assert_eq!(std::fs::read(&declared)?, b"shared asset bytes");

        std::fs::remove_file(&declared).ok();
        std::fs::remove_dir_all(&source_dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_pre_render_resolves_dependency_from_asset_source_before_verifying(
    ) -> Result<()> {
        // Regression test: execute_pre_render is the real public entry
        // point (not resolve_dependencies called directly, as in
        // test_add_asset_source_resolves_missing_dependency_from_local_source
        // above). It must run dependency resolution *before* asset
        // verification, or a dependency a configured asset source can
        // supply gets verified against its pre-resolution (missing) state
        // and the whole stage is wrongly marked Failed even though
        // resolution succeeded a line later.
        let source_dir = tmp_path("pre-render-asset-source-dir");
        std::fs::create_dir_all(&source_dir)?;
        std::fs::write(
            source_dir.join("pre_render_shared.bin"),
            b"resolved before verification",
        )?;

        let mut pipeline = Pipeline::new();
        pipeline.add_asset_source(source_dir.to_string_lossy());

        let declared = tmp_path("pre-render-asset-source-dest").join("pre_render_shared.bin");
        std::fs::remove_file(&declared).ok();

        let project_file = tmp_path("pre-render-asset-source.blend");
        std::fs::write(&project_file, b"pretend project file")?;

        let submission = JobSubmission::builder()
            .project_file(project_file.clone())
            .frame_range(1, 1)
            .dependency(declared.clone())
            .build()?;
        let job = Job::new(submission);

        let result = pipeline.execute_pre_render(&job).await?;
        assert!(
            result.assets_verified,
            "assets_verified must be true: the dependency was resolvable from the configured \
             asset source before verification ran"
        );
        assert!(result.dependencies_resolved);
        assert!(
            result.issues.is_empty(),
            "expected no issues, got: {:?}",
            result.issues
        );
        assert!(declared.exists());

        let tasks = pipeline.get_tasks(job.id);
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].status,
            TaskStatus::Completed,
            "a resolvable dependency must not fail the pre-render stage"
        );

        std::fs::remove_file(&declared).ok();
        std::fs::remove_file(&project_file).ok();
        std::fs::remove_dir_all(&source_dir).ok();
        Ok(())
    }
}
