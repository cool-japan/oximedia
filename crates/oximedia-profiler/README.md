# oximedia-profiler

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Performance profiling and optimization tools for OxiMedia. Provides comprehensive profiling capabilities including CPU, memory, GPU, frame timing, bottleneck detection, cache analysis, and flame graph generation.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **CPU profiling** - Sampling and instrumentation-based CPU profiling
- **Memory tracking** - Allocation tracking and leak detection
- **GPU profiling** - GPU timeline analysis and utilization
- **Frame timing** - Frame budget analysis and deadline tracking
- **Bottleneck detection** - Automatic bottleneck classification
- **Cache analysis** - Cache miss profiling and analysis
- **Thread analysis** - Thread utilization and contention detection
- **Flame graph generation** - Visual call stack analysis
- **Automated benchmarking** - Regression detection
- **Optimization suggestions** - Automated performance recommendations
- **Network profiling** - Network I/O profiling
- **Pipeline profiling** - Media pipeline stage profiling
- **Latency profiling** - Per-stage latency measurement
- **Throughput profiling** - Encoding throughput analysis
- **Codec profiling** - Codec-specific profiling
- **Sampling profiler** - Statistical sampling profiler
- **Memory fragmentation** - Memory fragmentation analysis
- **Resource tracking** - File and network resource tracking
- **Report formats** - Multiple report output formats

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-profiler = "0.2.0"
```

```rust
use oximedia_profiler::{Profiler, ProfilingMode, ProfileMetric};

let mut profiler = Profiler::new(ProfilingMode::Sampling);
profiler.start()?;

// Your code here

profiler.stop()?;
let report = profiler.generate_report();
println!("{}", report);

// Record custom metrics
profiler.record_metric("frames_encoded".to_string(), ProfileMetric::Count(1000));
```

## API Overview

**Core types:**
- `Profiler` — Main profiler with start/stop/report interface
- `ProfilingMode` — Sampling, Instrumentation, EventBased, Continuous
- `ProfilerConfig` — Configure which subsystems to profile
- `ProfileMetric` — Metric types: Duration, Count, Percentage, Bytes, Custom

**Modules:**
- `allocation_tracker` — Memory allocation tracking
- `benchmark` — Automated benchmarking
- `bottleneck` — Bottleneck detection and classification
- `cache` — Cache miss profiling
- `call_graph` — Call graph generation
- `codec_profiler` — Codec-specific profiling
- `cpu` — CPU profiling (sampling, per-CPU metrics)
- `event_trace` — Event-based tracing
- `flame`, `flamegraph` — Flame graph generation
- `frame`, `frame_profiler` — Frame timing and budget
- `gpu` — GPU profiling (memory, timeline)
- `hotspot` — Hotspot detection
- `latency_profiler` — Per-stage latency measurement
- `mem_profile`, `memory`, `memory_profiler` — Memory profiling
- `network_profiler` — Network I/O profiling
- `optimize` — Optimization suggestions
- `pipeline_profiler` — Media pipeline profiling
- `regression` — Performance regression detection
- `report`, `report_format` — Report generation and formatting
- `resource` — Resource tracking (files, network)
- `sampling_profiler` — Statistical sampling profiler
- `thread` — Thread utilization and contention
- `throughput_profiler` — Throughput analysis

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
