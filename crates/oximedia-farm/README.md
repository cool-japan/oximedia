# oximedia-farm

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Production-grade distributed encoding farm coordinator for OxiMedia, providing comprehensive job management, worker orchestration, and fault-tolerant distributed media processing.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

OxiMedia Farm coordinates video transcoding, quality control, and media analysis tasks across multiple worker nodes. It provides enterprise-grade features including priority-based job queuing, intelligent load balancing, automatic fault recovery, and Prometheus metrics.

## Features

- **Distributed Job Management** — Priority-based job queue with intelligent task distribution
- **Worker Orchestration** — Automatic registration, health monitoring, and capability tracking
- **Load Balancing** — Round-robin, least-loaded, capability-based, and deadline-aware routing
- **Fault Tolerance** — Automatic retry with exponential backoff, circuit breakers, graceful degradation
- **Real-time Monitoring** — Prometheus metrics and structured logging
- **Capacity Planning** — Resource capacity planner for optimal job sizing
- **Node Affinity** — Route jobs to workers with specific capabilities or tags
- **Task Preemption** — Preempt lower-priority tasks when high-priority jobs arrive
- **Job Templates** — Reusable job template definitions
- **Checkpointing** — Persistent state checkpoints for recovery
- **Secure Communication** — gRPC with optional TLS encryption
- **Persistence** — SQLite coordinator state and job history

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-farm = "0.2.0"
```

### Starting a Coordinator

```rust
use oximedia_farm::{Coordinator, CoordinatorConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = CoordinatorConfig {
        bind_address: "0.0.0.0:50051".to_string(),
        database_path: "farm.db".to_string(),
        heartbeat_timeout: std::time::Duration::from_secs(60),
        max_concurrent_jobs: 1000,
        enable_metrics: true,
        metrics_port: 9090,
        ..Default::default()
    };

    let coordinator = std::sync::Arc::new(Coordinator::new(config).await?);
    coordinator.start().await?;
    Ok(())
}
```

### Starting a Worker

```rust
use oximedia_farm::{Worker, WorkerConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = WorkerConfig {
        coordinator_address: "http://localhost:50051".to_string(),
        max_concurrent_tasks: 4,
        enable_gpu: true,
        ..Default::default()
    };

    let worker = std::sync::Arc::new(Worker::new(config));
    worker.start().await?;
    Ok(())
}
```

## API Overview

**Core types:**
- `Coordinator` — Central coordinator managing the farm
- `Worker` — Worker node agent
- `CoordinatorConfig` — Coordinator configuration
- `WorkerConfig` — Worker configuration
- `Job`, `Task` — Job and task definitions
- `JobType` — VideoTranscode / AudioTranscode / Thumbnail / QcValidation / Analysis
- `Priority` — Low / Normal / High / Critical

**Modules:**
- `coordinator` — Coordinator service implementation
- `worker`, `worker_pool` — Worker node and pool management
- `scheduler` — Job scheduling with multiple policies
- `job_queue` — Priority job queue
- `priority_queue` — Priority-ordered task queue
- `task_allocator` — Task allocation to workers
- `task_preemption` — Task preemption support
- `load_balancer` (via `coordinator`) — Load balancing strategies
- `farm_config` — Farm configuration types
- `resource_manager` — CPU/memory/GPU resource tracking
- `dependency` — Job dependency graph
- `node_monitor` — Worker health monitoring
- `heartbeat` (via `node_monitor`) — Heartbeat tracking
- `health` — Health status types
- `fault_tolerance` — Fault detection and recovery
- `circuit_breaker` (via `fault_tolerance`) — Circuit breaker pattern
- `checkpoint` — State checkpointing
- `persistence` — SQLite persistence layer
- `metrics` — Prometheus metrics collection
- `render_stats` — Render statistics tracking
- `farm_metrics` (via `metrics`) — Farm-level aggregated metrics
- `capacity_planner` — Resource capacity planning
- `node_affinity` — Worker affinity rules
- `job_template` — Reusable job templates
- `communication` — gRPC communication layer
- `worker_pool` — Worker pool management
- `pb` — Protocol buffer generated types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
