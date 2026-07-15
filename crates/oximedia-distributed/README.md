# oximedia-distributed

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Production-grade distributed video encoding coordinator for OxiMedia, providing horizontally scalable job distribution across worker nodes with fault tolerance and load balancing.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-distributed` implements a horizontally scalable distributed encoding coordinator capable of managing tens to hundreds of worker nodes with fault tolerance, Raft-based consensus, and low coordination overhead.

## Architecture

```
┌─────────────────┐
│   Coordinator   │ ◄─── gRPC ───┐
│  (Central Hub)  │              │
└────────┬────────┘              │
         │                       │
    ┌────┴────┐            ┌─────┴──────┐
    │         │            │            │
┌───▼───┐ ┌──▼────┐   ┌───▼───┐   ┌───▼───┐
│Worker │ │Worker │   │Worker │   │Worker │
│  #1   │ │  #2   │   │  #3   │   │  #N   │
└───────┘ └───────┘   └───────┘   └───────┘
```

## Features

- **Distributed Coordination** — Central coordinator managing the worker pool with Raft consensus
- **Job Scheduling** — Priority-based, FIFO, EDF, Fair Share, and SJF policies
- **Load Balancing** — Least-loaded, round-robin, fastest-first, and capability-based strategies
- **Fault Tolerance** — Automatic job rescheduling, heartbeat monitoring, circuit breakers, checkpointing
- **Worker Discovery** — Static, mDNS, etcd, and Consul registration methods
- **Segmentation** — Time-based, tile-based, and GOP-aligned video splitting
- **Back-pressure** — Flow control to prevent coordinator overload
- **Work Stealing** — Idle workers steal tasks from overloaded peers
- **Leader Election** — Raft-based leader election for coordinator HA
- **Metrics** — Real-time throughput, latency, and utilization tracking

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-distributed = "0.2.0"
```

```rust
use oximedia_distributed::{
    DistributedEncoder, DistributedConfig, DistributedJob,
    EncodingParams, SplitStrategy, JobPriority,
};
use std::time::Duration;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = DistributedConfig {
        coordinator_addr: "http://127.0.0.1:50051".to_string(),
        max_retries: 3,
        heartbeat_interval: Duration::from_secs(30),
        job_timeout: Duration::from_secs(3600),
        max_concurrent_jobs: 4,
        fault_tolerance: true,
        ..Default::default()
    };

    let encoder = DistributedEncoder::new(config);

    let job = DistributedJob {
        id: Uuid::new_v4(),
        task_id: Uuid::new_v4(),
        source_url: "s3://videos/input.mp4".to_string(),
        codec: "av1".to_string(),
        strategy: SplitStrategy::SegmentBased,
        priority: JobPriority::Normal,
        params: EncodingParams {
            bitrate: Some(5_000_000),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        },
        output_url: "s3://videos/output.mp4".to_string(),
        deadline: None,
    };

    let job_id = encoder.submit_job(job).await?;
    println!("Job submitted: {}", job_id);
    Ok(())
}
```

## API Overview

**Core types:**
- `DistributedEncoder` — Main distributed encoding interface
- `DistributedConfig` — Configuration (coordinator address, timeouts, retries)
- `DistributedJob` — Job definition with source, codec, strategy, and output
- `EncodingParams` — Encoding parameters (bitrate, resolution, preset)
- `SplitStrategy` — SegmentBased / TileBased / GopBased
- `JobPriority` — Low / Normal / High / Critical

**Modules:**
- `coordinator` — Central coordinator service
- `worker` — Worker node implementation
- `scheduler` — Advanced job scheduling
- `segment` — Video segmentation and reassembly
- `discovery` — Worker discovery (static, mDNS, etcd, Consul)
- `cluster` — Cluster membership management
- `consensus` — Raft-based consensus primitives
- `raft_primitives` — Raft log and state machine
- `leader_election` — Leader election protocol
- `load_balancer` — Load balancing strategies
- `fault_tolerance` — Fault detection and recovery
- `heartbeat` — Worker heartbeat tracking
- `backpressure` — Flow control and back-pressure
- `work_stealing` — Work-stealing scheduler
- `checkpointing` — State checkpointing
- `replication` — State replication
- `circuit_breaker` — Circuit breaker pattern
- `message_bus`, `message_queue` — Inter-node messaging
- `node_registry`, `node_health`, `node_topology` — Node management
- `task_distribution`, `task_queue`, `task_retry` — Task lifecycle
- `task_priority_queue` — Priority-ordered task queue
- `job_tracker` — Job progress tracking
- `resource_quota` — Per-job resource limits
- `partition`, `shard`, `shard_map` — Data partitioning
- `snapshot_store` — Cluster snapshot persistence
- `metrics_aggregator` — Metrics collection and aggregation
- `pb` — Protocol buffer generated types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
