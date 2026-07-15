# oximedia-batch

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Comprehensive batch processing engine for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Core Batch Processing
- **Job Definition** — Templates for transcode, QC, analysis, and file operations
- **Job Queue** — Priority-based queue with job dependencies and scheduling
- **Execution Engine** — Worker pool management with resource allocation and fault tolerance

### Batch Operations
- **Media Operations** — Transcoding, quality control, analysis
- **File Operations** — Copy, move, rename, archive, checksum
- **Transformation Pipelines** — Multi-step processing with conditional branching

### Template System
- **Variable Substitution** — File properties, date/time, media properties, custom variables
- **Template Syntax** — Conditionals, loops, functions
- **Preset Templates** — Web, mobile, broadcast, archive presets

### Monitoring and Reporting
- **Progress Tracking** — Per-job and overall batch progress
- **Status Reporting** — Real-time job status updates
- **Report Generation** — JSON, CSV, HTML export formats
- **Prometheus Metrics** — Built-in metrics endpoint

### Advanced Features
- **Watch Folders** — Automatic job submission for new files (via notify)
- **Distributed Processing** — Network worker support (integrates with oximedia-farm)
- **Scripting Support** — Lua 5.4 scripting for custom logic (mlua, vendored)
- **REST API** — axum-based HTTP API for remote job management
- **Notifications** — Email, Slack, Discord, Teams, webhooks (via reqwest)

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-batch = "0.2.0"
```

### Basic Example

```rust
use oximedia_batch::{BatchEngine, BatchJob, BatchOperation};
use oximedia_batch::operations::FileOperation;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Arc::new(BatchEngine::new("batch.db", 4)?);
    engine.start().await?;

    let mut job = BatchJob::new(
        "Copy Files".to_string(),
        BatchOperation::FileOp {
            operation: FileOperation::Copy { overwrite: false },
        },
    );
    job.add_input(oximedia_batch::InputSpec::new("*.mp4".to_string()));
    job.add_output(oximedia_batch::OutputSpec::new(
        "output/{filename}".to_string(),
        oximedia_batch::operations::OutputFormat::Mp4,
    ));

    let job_id = engine.submit_job(job).await?;
    println!("Submitted job: {}", job_id);
    Ok(())
}
```

### Job Configuration

```rust
let mut job = BatchJob::new(
    "My Job".to_string(),
    BatchOperation::Transcode { preset: "web".to_string() },
);
job.set_priority(Priority::High);
job.set_retry_policy(RetryPolicy::new(3, 60, true));
job.set_schedule(Schedule::At(future_time));
job.set_resources(ResourceRequirements {
    cpu_cores: Some(4),
    memory_mb: Some(4096),
    gpu: true,
    disk_space_mb: Some(10240),
});
```

### REST API

```bash
# Submit job
curl -X POST http://localhost:3000/api/v1/jobs -H "Content-Type: application/json" -d @job.json

# Get job status
curl http://localhost:3000/api/v1/jobs/<job-id>

# Cancel job
curl -X DELETE http://localhost:3000/api/v1/jobs/<job-id>
```

## Architecture (51 source files, 600 public items)

**Components:**
- `BatchEngine` — Main entry point, coordinates queue, engine, and database
- `JobQueue` — Priority-based queue with scheduling support
- `ExecutionEngine` — Manages worker pool and job execution
- `Database` — SQLite persistence for jobs, logs, and results (rusqlite + r2d2)
- `Template` — Variable substitution and template rendering
- `Monitoring` — Progress tracking and Prometheus metrics
- `Notifications` — Multi-channel notification system

**Job Lifecycle:**
1. Job submitted to queue
2. Scheduler checks schedule and dependencies
3. Worker picks up job when ready
4. Operation executor processes the job
5. Results saved to SQLite database
6. Notifications sent
7. Metrics updated

## Integrations

- `oximedia-transcode` — Transcoding operations
- `oximedia-qc` — Quality control checks
- `oximedia-workflow` — Orchestration
- `oximedia-farm` — Distributed processing
- `oximedia-monitor` — Metrics and monitoring

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
