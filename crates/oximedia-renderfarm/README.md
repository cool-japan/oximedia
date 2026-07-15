# oximedia-renderfarm

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Enterprise-grade render farm coordinator for OxiMedia. Provides distributed media rendering with job management, worker pools, advanced scheduling, cloud bursting, fault tolerance, and real-time monitoring.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Job Management** - Submit, track, and manage render jobs with priority levels
- **Worker Management** - Auto-discovery, health monitoring, and pool organization
- **Task Distribution** - Advanced scheduling algorithms and load balancing
- **Rendering Pipeline** - Pre-render, render, and post-render phases
- **Storage Management** - Distributed storage, asset distribution, and caching
- **Monitoring** - Real-time monitoring, dashboards, and alerts
- **Cost Management** - Cost tracking, budget management, and billing
- **Cloud Integration** - Hybrid rendering with cloud bursting
- **Fault Tolerance** - Automatic retry, checkpointing, and recovery
- **Tile Rendering** - Distributed tile-based rendering
- **Preview System** - Live render preview
- **Deadline Integration** - Third-party render farm integration (Deadline)
- **Prometheus Metrics** - Built-in metrics export
- **Blade Compute** - Blade render node management
- **License Server** - Software license server integration
- **Frame Distribution** - Efficient frame distribution across workers
- **Job Archives** - Job archiving and retrieval
- **Render Quotas** - Resource quota management
- **Output Validation** - Rendered output validation
- **Dependency Graph** - Job dependency management
- **Priority Queue** - Advanced priority queue for job scheduling
- **Node Heartbeat** - Worker node health via heartbeat
- **Render Manifest** - Render job manifest generation
- **Simulation** - Farm simulation for capacity planning

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-renderfarm = "0.2.0"
```

```rust
use oximedia_renderfarm::{Coordinator, CoordinatorConfig, JobSubmission, Priority};

// Create coordinator
let config = CoordinatorConfig::default();
let coordinator = Coordinator::new(config).await?;

// Submit a render job
let job = JobSubmission::builder()
    .project_file("path/to/project.blend")
    .frame_range(1, 100)
    .priority(Priority::High)
    .build()?;

let job_id = coordinator.submit_job(job).await?;
```

## API Overview

**Core types:**
- `Coordinator` / `CoordinatorConfig` — Main render farm coordinator
- `Job` / `JobId` / `JobSubmission` / `JobState` / `Priority` — Job lifecycle management
- `Worker` / `WorkerId` / `WorkerCapabilities` / `WorkerState` — Worker nodes
- `WorkerPool` / `PoolId` — Worker pool management
- `Scheduler` / `SchedulingAlgorithm` — Job scheduling strategies
- `Monitor` / `MonitorConfig` — Real-time monitoring
- `CostTracker` / `CostReport` — Cost management
- `RenderFarmApi` / `ApiConfig` — REST API interface

**Modules:**
- `api` — REST API interface
- `assets` — Asset management
- `blade_compute` — Blade render node management
- `budget` — Budget management
- `cache` — Render cache
- `cloud` — Cloud bursting integration
- `coordinator` — Main coordinator
- `cost`, `cost_optimizer` — Cost tracking and optimization
- `dashboard` — Monitoring dashboard
- `deadline_integration`, `deadline_scheduler` — Deadline integration
- `dependency` — Job dependency tracking
- `distribution`, `frame_distribution` — Frame distribution
- `error` — Error types
- `events` — Event system
- `farm_metrics` — Farm-level metrics
- `health` — Health monitoring
- `job` — Job management
- `job_archive` — Job archiving
- `job_dependency_graph` — Dependency graph
- `job_priority_queue` — Priority queue
- `license_server` — License server integration
- `load_balancer` — Load balancing
- `monitoring` — Real-time monitoring
- `node_capability` — Node capability detection
- `node_heartbeat` — Node health monitoring
- `node_pool` — Node pool management
- `output_validator` — Output validation
- `pipeline` — Render pipeline
- `plugin` — Plugin system
- `pool` — Worker pool
- `preview` — Live render preview
- `priority_queue` — Priority queue
- `progress` — Progress tracking
- `recovery` — Fault recovery
- `render_job_queue` — Job queue
- `render_log` — Render logging
- `render_manifest` — Job manifest
- `render_node_status` — Node status tracking
- `render_priority` — Priority management
- `render_quota` — Resource quotas
- `reporting` — Report generation
- `scheduler` — Job scheduler
- `simulation` — Farm simulation
- `stats` — Statistics
- `storage` — Distributed storage
- `sync` — Synchronization
- `tile_render`, `tile_rendering` — Tile-based rendering
- `verification` — Job verification
- `worker` — Worker management

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
