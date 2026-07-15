# oximedia-jobs

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 499](https://img.shields.io/badge/tests-499-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

Production-quality job queue and worker management system for OxiMedia video transcoding operations.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Priority Queue** — Three-level priority system (High, Normal, Low) with boost support
- **Persistent Storage** — SQLite-based job persistence
- **Worker Pool** — Configurable worker pool with auto-scaling and load balancing
- **Job Scheduling** — Schedule jobs for future execution with recurrence support
- **Scheduling Rules** — Fine-grained scheduling rule engine
- **Dependencies** — Job dependency graph for ordered execution
- **Retry Logic** — Automatic retry with exponential backoff and retry policy
- **Job Cancellation** — Cancel running or pending jobs
- **Conditional Execution** — Execute based on conditions (OnSuccess, OnFailure, AnySuccess)
- **Resource Quotas** — Limit CPU, memory, GPU, and execution time per job
- **Resource Claims** — Reserve resources before job execution
- **Resource Estimates** — Estimate resource requirements for scheduling
- **Resource Limits** — Hard resource limits per job
- **Deadline Scheduling** — Set deadlines for job completion
- **Health Monitoring** — Worker health checks and auto-recovery
- **Comprehensive Metrics** — Throughput, latency, utilization tracking
- **Job Templates** — Reusable job configuration templates
- **Job Tags** — Tag-based job filtering and routing
- **Job History** — Persistent job execution history
- **Job Graph Visualization** — Export dependency graphs
- **Throughput Tracking** — Real-time throughput measurement
- **Event Log** — Job lifecycle event log
- **Telemetry** — Structured telemetry for observability

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-jobs = "0.2.0"
```

```rust
use oximedia_jobs::{
    Job, JobPayload, Priority, TranscodeParams,
    JobQueue, QueueConfig, WorkerConfig, MetricsCollector,
    DefaultExecutor,
};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let metrics = Arc::new(MetricsCollector::new());
    let executor = Arc::new(DefaultExecutor);

    let queue_config = QueueConfig {
        db_path: "jobs.db".to_string(),
        max_concurrent_jobs: 10,
        enable_retry: true,
        ..Default::default()
    };

    let worker_config = WorkerConfig {
        pool_size: 4,
        auto_scaling: true,
        max_pool_size: 16,
        ..Default::default()
    };

    let queue = JobQueue::new(queue_config, executor, metrics, worker_config)
        .expect("Failed to create queue");
    queue.start().await;

    let params = TranscodeParams {
        input: "input.mp4".to_string(),
        output: "output.webm".to_string(),
        video_codec: "av1".to_string(),
        audio_codec: "opus".to_string(),
        video_bitrate: 5_000_000,
        ..Default::default()
    };

    let job = Job::new(
        "Transcode video".to_string(),
        Priority::High,
        JobPayload::Transcode(params),
    );

    let job_id = queue.submit(job).await.expect("Failed to submit job");
    println!("Submitted job: {}", job_id);

    queue.shutdown().await;
}
```

## API Overview

**Core types:**
- `Job` / `JobBuilder` — Job definition and builder
- `JobPayload` — Transcode / Thumbnail / SpriteSheet / Analysis / Batch
- `Priority` — High / Normal / Low
- `JobQueue` — Main queue with persistence and worker pool
- `QueueConfig` — Queue configuration
- `WorkerConfig` — Worker pool configuration
- `MetricsCollector` — Performance metrics
- `DefaultExecutor` — Default job executor

**Modules:**
- `job` — Job definitions, priorities, execution logic
- `queue` — Priority queue with dependency management
- `worker`, `worker_pool` — Worker pool and execution
- `scheduler` — Advanced scheduling
- `scheduling_rule` — Scheduling rule engine
- `dependency`, `dependency_graph` — Job dependency graph
- `retry`, `retry_policy` — Retry with exponential backoff
- `persistence` — SQLite persistence layer
- `metrics`, `job_metrics` — Metrics collection
- `priority` — Priority levels
- `job_priority_boost` — Dynamic priority boosting
- `quota`, `resource_quota` — Resource quota management
- `resource_claim` — Resource reservation
- `resource_estimate` — Resource estimation
- `resource_limits` — Hard resource limits
- `rate_limiter` — Job submission rate limiting
- `batch` — Batch job processing
- `registry` — Job type registry
- `job_template` — Job template definitions
- `job_tags` — Tag-based routing
- `job_history` — Execution history
- `job_graph_viz` — Dependency graph visualization
- `throughput_tracker` — Real-time throughput measurement
- `event_log` — Job lifecycle event log
- `telemetry` — Structured telemetry

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
