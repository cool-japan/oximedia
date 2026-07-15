# oximedia-workflow

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Comprehensive workflow orchestration engine for OxiMedia. Provides DAG-based workflow definition, task dependencies, parallel execution, state persistence, scheduling, REST API, and real-time monitoring.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Core Engine
- **DAG-based Workflows** - Define complex workflows as directed acyclic graphs
- **Task Dependencies** - Automatic dependency resolution and topological sorting
- **Parallel Execution** - Run independent tasks concurrently with configurable limits
- **State Persistence** - SQLite-based workflow state management
- **Retry Logic** - Automatic retry with exponential backoff and configurable policies
- **Conditional Branching** - Execute tasks based on runtime conditions
- **Step Conditions** - Fine-grained conditional execution per step
- **Approval Gates** - Manual approval checkpoints within workflows
- **SLA Tracking** - Service level agreement monitoring and alerting
- **Cost Tracking** - Resource cost accumulation per workflow and task
- **Workflow Versioning** - Track workflow definition changes over time
- **Workflow Snapshots** - Save and restore workflow execution snapshots
- **Checkpointing** - Resume interrupted workflows from checkpoints
- **Throttling** - Rate limiting for workflow execution

### Task Types
- **Transcode** - Media transcoding with preset selection
- **QC Validation** - Quality control checks
- **File Transfer** - Local, FTP, SFTP, S3, HTTP transfers
- **Notifications** - Email, Webhook, Slack, Discord
- **Custom Scripts** - Execute arbitrary scripts
- **Media Analysis** - Scene detection, quality analysis, and more
- **HTTP Requests** - Make HTTP/REST calls
- **Wait** - Simple delay tasks

### Scheduling
- **Cron Schedules** - Standard cron expressions
- **Watch Folders** - Trigger on file arrival
- **API Triggers** - Manual or programmatic execution
- **Time-based** - One-time scheduled execution
- **Event-based** - Custom event triggers
- **Interval** - Periodic execution
- **Manual** - Explicit on-demand triggering

### Monitoring
- **Real-time Progress** - Track workflow and task execution
- **Metrics Collection** - Duration, throughput, success rates
- **Execution History** - Historical workflow data
- **System Statistics** - Aggregate metrics
- **Audit Log** - Complete workflow audit trail
- **Workflow Log** - Structured per-workflow execution log

### Integration
- **REST API** - Complete HTTP API for workflow management
- **WebSocket** - Real-time updates and event streaming
- **CLI** - Command-line interface
- **Extensible** - Custom task executors

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-workflow = "0.2.0"
```

```rust
use oximedia_workflow::{WorkflowEngine, Workflow, Task, TaskType};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create engine
    let engine = WorkflowEngine::new("workflows.db")?;

    // Define workflow
    let mut workflow = Workflow::new("simple-workflow");
    let task = Task::new("process", TaskType::Wait {
        duration: Duration::from_secs(5),
    });
    workflow.add_task(task);

    // Submit and execute
    let workflow_id = engine.submit_workflow(&workflow).await?;
    engine.execute_workflow(workflow_id).await?;

    Ok(())
}
```

### Workflow Patterns

```rust
use oximedia_workflow::patterns::watch_folder_transcode;
use std::path::PathBuf;

// Watch folder auto-transcode
let workflow = watch_folder_transcode(
    PathBuf::from("/input"),
    PathBuf::from("/output"),
    "h264".to_string(),
);

// Multi-pass encoding workflow
use oximedia_workflow::patterns::multi_pass_encoding;
let workflow = multi_pass_encoding(
    PathBuf::from("/source.mp4"),
    PathBuf::from("/proxy.mp4"),
    PathBuf::from("/final.mp4"),
    "broadcast".to_string(),
);

// Validation pipeline
use oximedia_workflow::patterns::validation_pipeline;
let workflow = validation_pipeline(
    PathBuf::from("/input.mp4"),
    "s3://archive/bucket".to_string(),
    vec!["admin@example.com".to_string()],
);
```

### Scheduling

```rust
use oximedia_workflow::{WorkflowEngine, Trigger};

let trigger = Trigger::Cron {
    expression: "0 0 * * * *".to_string(),
    timezone: "UTC".to_string(),
};
engine.schedule_workflow(workflow, trigger).await?;
```

## REST API Endpoints

- `POST /api/v1/workflows` — Create workflow
- `GET /api/v1/workflows` — List workflows
- `GET /api/v1/workflows/:id` — Get workflow
- `POST /api/v1/workflows/:id/execute` — Execute workflow
- `GET /api/v1/workflows/:id/status` — Get workflow status
- `POST /api/v1/schedules` — Create schedule
- `GET /api/v1/monitoring/statistics` — Get statistics

## API Overview

- `WorkflowEngine` — Main engine: new(), in_memory(), submit_workflow(), execute_workflow(), schedule_workflow(), start(), stop(), api_router()
- `Workflow` / `WorkflowId` / `WorkflowState` / `WorkflowConfig` / `Edge` — Workflow data model
- `Task` / `TaskId` / `TaskType` / `TaskState` / `TaskResult` / `TaskPriority` / `RetryPolicy` — Task management
- `WorkflowExecutor` / `DefaultTaskExecutor` / `TaskExecutor` / `ExecutionContext` — Execution engine
- `WorkflowScheduler` / `ScheduledWorkflow` / `Trigger` / `FileWatcher` — Scheduling
- `MonitoringService` / `WorkflowMetrics` / `TaskMetrics` / `SystemStatistics` — Monitoring
- `PersistenceManager` — SQLite-backed workflow state
- `TaskQueue` / `QueueStatistics` — Task queuing
- `WorkflowBuilder` / `TaskBuilder` / `TranscodeTaskBuilder` / `QcTaskBuilder` / `TransferTaskBuilder` — Builder APIs
- `WorkflowDag` / `DagWorkflowEngine` / `WorkflowNode` / `WorkflowEdge` / `WorkflowTemplate` — DAG engine
- `WorkflowValidator` / `TaskValidator` / `ValidationReport` / `ComplexityAnalyzer` — Validation
- `WebSocketManager` / `WorkflowEvent` / `WebSocketState` — WebSocket integration
- `WorkflowError` / `Result` — Error and result types
- Modules: `api`, `approval_gate`, `audit_log`, `builder`, `cli`, `cost_tracking`, `dag`, `error`, `executor`, `monitoring`, `notification_system`, `patterns`, `persistence`, `queue`, `resource_pool`, `retry_policy`, `scheduler`, `sla`, `sla_tracking`, `state_machine`, `step_condition`, `step_result`, `task`, `task_dependency`, `task_graph`, `task_priority_queue`, `task_template`, `templates`, `triggers`, `utils`, `validation`, `websocket`, `workflow`, `workflow_audit`, `workflow_checkpoint`, `workflow_log`, `workflow_metrics`, `workflow_snapshot`, `workflow_throttle`, `workflow_version`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
