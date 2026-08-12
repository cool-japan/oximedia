// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Job queue metrics and monitoring.

use crate::job::{JobStatus, Priority};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Job execution metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMetrics {
    /// Job ID
    pub job_id: Uuid,
    /// Job name
    pub job_name: String,
    /// Job type
    pub job_type: String,
    /// Priority
    pub priority: Priority,
    /// Start time
    pub started_at: DateTime<Utc>,
    /// End time
    pub completed_at: Option<DateTime<Utc>>,
    /// Duration in seconds
    pub duration_secs: Option<f64>,
    /// Status
    pub status: JobStatus,
    /// Number of attempts
    pub attempts: u32,
    /// Worker ID
    pub worker_id: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
}

impl JobMetrics {
    /// Create new job metrics
    #[must_use]
    pub fn new(
        job_id: Uuid,
        job_name: String,
        job_type: String,
        priority: Priority,
        worker_id: Option<String>,
    ) -> Self {
        Self {
            job_id,
            job_name,
            job_type,
            priority,
            started_at: Utc::now(),
            completed_at: None,
            duration_secs: None,
            status: JobStatus::Running,
            attempts: 1,
            worker_id,
            error: None,
        }
    }

    /// Mark as completed
    pub fn mark_completed(&mut self) {
        self.completed_at = Some(Utc::now());
        self.status = JobStatus::Completed;
        self.calculate_duration();
    }

    /// Mark as failed
    pub fn mark_failed(&mut self, error: String) {
        self.completed_at = Some(Utc::now());
        self.status = JobStatus::Failed;
        self.error = Some(error);
        self.calculate_duration();
    }

    /// Calculate duration
    fn calculate_duration(&mut self) {
        if let Some(completed) = self.completed_at {
            let duration = completed.signed_duration_since(self.started_at);
            #[allow(clippy::cast_precision_loss)]
            {
                self.duration_secs = Some(duration.num_milliseconds() as f64 / 1000.0);
            }
        }
    }
}

/// Worker metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerMetrics {
    /// Worker ID
    pub worker_id: String,
    /// Worker status
    pub status: WorkerStatus,
    /// Number of jobs processed
    pub jobs_processed: u64,
    /// Number of jobs succeeded
    pub jobs_succeeded: u64,
    /// Number of jobs failed
    pub jobs_failed: u64,
    /// Current job ID
    pub current_job: Option<Uuid>,
    /// Start time
    pub started_at: DateTime<Utc>,
    /// Last heartbeat
    pub last_heartbeat: DateTime<Utc>,
    /// Total processing time in seconds
    pub total_processing_time_secs: f64,
}

/// Worker status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    /// Worker is idle
    Idle,
    /// Worker is busy
    Busy,
    /// Worker is stopped
    Stopped,
    /// Worker is unhealthy
    Unhealthy,
}

impl WorkerMetrics {
    /// Create new worker metrics
    #[must_use]
    pub fn new(worker_id: String) -> Self {
        Self {
            worker_id,
            status: WorkerStatus::Idle,
            jobs_processed: 0,
            jobs_succeeded: 0,
            jobs_failed: 0,
            current_job: None,
            started_at: Utc::now(),
            last_heartbeat: Utc::now(),
            total_processing_time_secs: 0.0,
        }
    }

    /// Update heartbeat
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = Utc::now();
    }

    /// Mark job started
    pub fn job_started(&mut self, job_id: Uuid) {
        self.status = WorkerStatus::Busy;
        self.current_job = Some(job_id);
    }

    /// Mark job completed
    pub fn job_completed(&mut self, duration_secs: f64) {
        self.status = WorkerStatus::Idle;
        self.current_job = None;
        self.jobs_processed += 1;
        self.jobs_succeeded += 1;
        self.total_processing_time_secs += duration_secs;
    }

    /// Mark job failed
    pub fn job_failed(&mut self, duration_secs: f64) {
        self.status = WorkerStatus::Idle;
        self.current_job = None;
        self.jobs_processed += 1;
        self.jobs_failed += 1;
        self.total_processing_time_secs += duration_secs;
    }

    /// Get success rate
    #[must_use]
    pub fn success_rate(&self) -> f64 {
        if self.jobs_processed == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                (self.jobs_succeeded as f64 / self.jobs_processed as f64) * 100.0
            }
        }
    }

    /// Get average processing time
    #[must_use]
    pub fn avg_processing_time(&self) -> f64 {
        if self.jobs_processed == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                self.total_processing_time_secs / self.jobs_processed as f64
            }
        }
    }

    /// Check if worker is healthy
    #[must_use]
    pub fn is_healthy(&self, timeout_secs: i64) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.last_heartbeat)
            .num_seconds();
        elapsed < timeout_secs
    }
}

/// Queue statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueueStats {
    /// Total jobs
    pub total_jobs: usize,
    /// Pending jobs
    pub pending_jobs: usize,
    /// Running jobs
    pub running_jobs: usize,
    /// Completed jobs
    pub completed_jobs: usize,
    /// Failed jobs
    pub failed_jobs: usize,
    /// Cancelled jobs
    pub cancelled_jobs: usize,
    /// Scheduled jobs
    pub scheduled_jobs: usize,
    /// Average queue time in seconds
    pub avg_queue_time_secs: f64,
    /// Average processing time in seconds
    pub avg_processing_time_secs: f64,
    /// Success rate percentage
    pub success_rate: f64,
}

impl QueueStats {
    /// Calculate success rate
    pub fn calculate_success_rate(&mut self) {
        let finished = self.completed_jobs + self.failed_jobs;
        if finished > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                self.success_rate = (self.completed_jobs as f64 / finished as f64) * 100.0;
            }
        } else {
            self.success_rate = 0.0;
        }
    }
}

/// Metrics collector
pub struct MetricsCollector {
    /// Job metrics history
    job_metrics: Arc<RwLock<Vec<JobMetrics>>>,
    /// Worker metrics
    worker_metrics: Arc<RwLock<HashMap<String, WorkerMetrics>>>,
    /// Queue statistics
    queue_stats: Arc<RwLock<QueueStats>>,
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector {
    /// Create a new metrics collector
    #[must_use]
    pub fn new() -> Self {
        Self {
            job_metrics: Arc::new(RwLock::new(Vec::new())),
            worker_metrics: Arc::new(RwLock::new(HashMap::new())),
            queue_stats: Arc::new(RwLock::new(QueueStats::default())),
        }
    }

    /// Record job started
    pub async fn record_job_started(
        &self,
        job_id: Uuid,
        job_name: String,
        job_type: String,
        priority: Priority,
        worker_id: String,
    ) {
        let mut metrics = JobMetrics::new(
            job_id,
            job_name,
            job_type,
            priority,
            Some(worker_id.clone()),
        );
        metrics.status = JobStatus::Running;

        let mut job_metrics = self.job_metrics.write().await;
        job_metrics.push(metrics);

        let mut worker_metrics = self.worker_metrics.write().await;
        if let Some(worker) = worker_metrics.get_mut(&worker_id) {
            worker.job_started(job_id);
        }
    }

    /// Record job completed
    pub async fn record_job_completed(&self, job_id: Uuid, worker_id: &str) {
        let mut job_metrics = self.job_metrics.write().await;
        if let Some(metrics) = job_metrics.iter_mut().find(|m| m.job_id == job_id) {
            metrics.mark_completed();

            if let Some(duration) = metrics.duration_secs {
                let mut worker_metrics = self.worker_metrics.write().await;
                if let Some(worker) = worker_metrics.get_mut(worker_id) {
                    worker.job_completed(duration);
                }
            }
        }
    }

    /// Record job failed
    pub async fn record_job_failed(&self, job_id: Uuid, worker_id: &str, error: String) {
        let mut job_metrics = self.job_metrics.write().await;
        if let Some(metrics) = job_metrics.iter_mut().find(|m| m.job_id == job_id) {
            metrics.mark_failed(error);

            if let Some(duration) = metrics.duration_secs {
                let mut worker_metrics = self.worker_metrics.write().await;
                if let Some(worker) = worker_metrics.get_mut(worker_id) {
                    worker.job_failed(duration);
                }
            }
        }
    }

    /// Register worker
    pub async fn register_worker(&self, worker_id: String) {
        let mut worker_metrics = self.worker_metrics.write().await;
        worker_metrics.insert(worker_id.clone(), WorkerMetrics::new(worker_id));
    }

    /// Unregister worker
    pub async fn unregister_worker(&self, worker_id: &str) {
        let mut worker_metrics = self.worker_metrics.write().await;
        if let Some(worker) = worker_metrics.get_mut(worker_id) {
            worker.status = WorkerStatus::Stopped;
        }
    }

    /// Update worker heartbeat
    pub async fn worker_heartbeat(&self, worker_id: &str) {
        let mut worker_metrics = self.worker_metrics.write().await;
        if let Some(worker) = worker_metrics.get_mut(worker_id) {
            worker.heartbeat();
        }
    }

    /// Update queue statistics
    pub async fn update_queue_stats(&self, stats: QueueStats) {
        let mut queue_stats = self.queue_stats.write().await;
        *queue_stats = stats;
    }

    /// Get job metrics
    pub async fn get_job_metrics(&self) -> Vec<JobMetrics> {
        self.job_metrics.read().await.clone()
    }

    /// Get job metrics by ID
    pub async fn get_job_metric(&self, job_id: Uuid) -> Option<JobMetrics> {
        let metrics = self.job_metrics.read().await;
        metrics.iter().find(|m| m.job_id == job_id).cloned()
    }

    /// Get worker metrics
    pub async fn get_worker_metrics(&self) -> HashMap<String, WorkerMetrics> {
        self.worker_metrics.read().await.clone()
    }

    /// Get worker metric by ID
    pub async fn get_worker_metric(&self, worker_id: &str) -> Option<WorkerMetrics> {
        let metrics = self.worker_metrics.read().await;
        metrics.get(worker_id).cloned()
    }

    /// Get queue statistics
    pub async fn get_queue_stats(&self) -> QueueStats {
        self.queue_stats.read().await.clone()
    }

    /// Get job metrics by status
    pub async fn get_job_metrics_by_status(&self, status: JobStatus) -> Vec<JobMetrics> {
        let metrics = self.job_metrics.read().await;
        metrics
            .iter()
            .filter(|m| m.status == status)
            .cloned()
            .collect()
    }

    /// Get job metrics by type
    pub async fn get_job_metrics_by_type(&self, job_type: &str) -> Vec<JobMetrics> {
        let metrics = self.job_metrics.read().await;
        metrics
            .iter()
            .filter(|m| m.job_type == job_type)
            .cloned()
            .collect()
    }

    /// Get average job duration by type
    #[must_use]
    pub async fn get_avg_duration_by_type(&self, job_type: &str) -> f64 {
        let metrics = self.job_metrics.read().await;
        let durations: Vec<f64> = metrics
            .iter()
            .filter(|m| m.job_type == job_type)
            .filter_map(|m| m.duration_secs)
            .collect();

        if durations.is_empty() {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                durations.iter().sum::<f64>() / durations.len() as f64
            }
        }
    }

    /// Get success rate by type
    #[must_use]
    pub async fn get_success_rate_by_type(&self, job_type: &str) -> f64 {
        let metrics = self.job_metrics.read().await;
        let type_metrics: Vec<&JobMetrics> =
            metrics.iter().filter(|m| m.job_type == job_type).collect();

        if type_metrics.is_empty() {
            return 0.0;
        }

        let succeeded = type_metrics
            .iter()
            .filter(|m| m.status == JobStatus::Completed)
            .count();

        #[allow(clippy::cast_precision_loss)]
        {
            (succeeded as f64 / type_metrics.len() as f64) * 100.0
        }
    }

    /// Clean old metrics
    pub async fn cleanup_old_metrics(&self, days: i64) {
        let cutoff = Utc::now() - chrono::Duration::days(days);

        let mut job_metrics = self.job_metrics.write().await;
        job_metrics.retain(|m| m.started_at > cutoff);
    }

    /// Get worker utilization
    pub async fn get_worker_utilization(&self) -> f64 {
        let worker_metrics = self.worker_metrics.read().await;
        if worker_metrics.is_empty() {
            return 0.0;
        }

        let busy_count = worker_metrics
            .values()
            .filter(|w| w.status == WorkerStatus::Busy)
            .count();

        (busy_count as f64 / worker_metrics.len() as f64) * 100.0
    }

    /// Get unhealthy workers
    pub async fn get_unhealthy_workers(&self, timeout_secs: i64) -> Vec<String> {
        let worker_metrics = self.worker_metrics.read().await;
        worker_metrics
            .iter()
            .filter(|(_, m)| !m.is_healthy(timeout_secs))
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Performance report
#[derive(Debug, Serialize, Deserialize)]
pub struct PerformanceReport {
    /// Report timestamp
    pub timestamp: DateTime<Utc>,
    /// Queue statistics
    pub queue_stats: QueueStats,
    /// Worker utilization percentage
    pub worker_utilization: f64,
    /// Average job duration by type
    pub avg_duration_by_type: HashMap<String, f64>,
    /// Success rate by type
    pub success_rate_by_type: HashMap<String, f64>,
    /// Top performers (workers)
    pub top_workers: Vec<(String, f64)>,
    /// Slow jobs (above threshold)
    pub slow_jobs: Vec<JobMetrics>,
}

impl PerformanceReport {
    /// Generate performance report
    pub async fn generate(collector: &MetricsCollector, slow_threshold_secs: f64) -> Self {
        let queue_stats = collector.get_queue_stats().await;
        let worker_utilization = collector.get_worker_utilization().await;
        let job_metrics = collector.get_job_metrics().await;

        let mut avg_duration_by_type: HashMap<String, f64> = HashMap::new();
        let mut success_rate_by_type: HashMap<String, f64> = HashMap::new();

        let job_types: Vec<String> = job_metrics
            .iter()
            .map(|m| m.job_type.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        for job_type in &job_types {
            avg_duration_by_type.insert(
                job_type.clone(),
                collector.get_avg_duration_by_type(job_type).await,
            );
            success_rate_by_type.insert(
                job_type.clone(),
                collector.get_success_rate_by_type(job_type).await,
            );
        }

        let worker_metrics = collector.get_worker_metrics().await;
        let mut top_workers: Vec<(String, f64)> = worker_metrics
            .iter()
            .map(|(id, m)| (id.clone(), m.success_rate()))
            .collect();
        top_workers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        top_workers.truncate(10);

        let slow_jobs: Vec<JobMetrics> = job_metrics
            .into_iter()
            .filter(|m| {
                if let Some(duration) = m.duration_secs {
                    duration > slow_threshold_secs
                } else {
                    false
                }
            })
            .collect();

        Self {
            timestamp: Utc::now(),
            queue_stats,
            worker_utilization,
            avg_duration_by_type,
            success_rate_by_type,
            top_workers,
            slow_jobs,
        }
    }
}
