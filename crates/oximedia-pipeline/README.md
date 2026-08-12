# oximedia-pipeline

**Status: [Stable]** | Version: 0.2.1 | Tests: extensively tested | Updated: 2026-08-12

[![Crates.io](https://img.shields.io/crates/v/oximedia-pipeline.svg)](https://crates.io/crates/oximedia-pipeline)
[![Docs.rs](https://docs.rs/oximedia-pipeline/badge.svg)](https://docs.rs/oximedia-pipeline)
[![License](https://img.shields.io/crates/l/oximedia-pipeline.svg)](https://github.com/cool-japan/oximedia)

Declarative media processing pipeline DSL for the
[OxiMedia](https://github.com/cool-japan/oximedia) framework.

Build typed filter graphs, chain nodes with a fluent API, validate the DAG at
compile-time intent and build-time structure, estimate resources, and optimise
the graph before execution -- all in pure Rust with zero C/Fortran dependencies.

## Features

- **Typed filter graph** -- nodes carry `StreamSpec` on every pad, so
  video-to-audio mis-connections are caught at graph-build time.
- **Fluent builder DSL** -- `PipelineBuilder` and `NodeChain` let you spell out
  a pipeline in a single expression with `.source()`, `.scale()`, `.hflip()`,
  `.sink()`, etc.
- **DAG validation** -- cycle detection (Kahn's algorithm), dangling-edge
  checks, and unconnected-pad checks via `PipelineGraph::validate()`.
- **Execution planning** -- `ExecutionPlanner` assigns level numbers and groups
  independent nodes into parallelisable stages.
- **Resource estimation** -- peak memory, recommended CPU threads, and GPU
  recommendation derived from the plan and the graph.
- **Graph optimisation** -- `PipelineOptimizer` fuses consecutive Scale filters
  and eliminates no-op identity filters (zero-gain Volume, equal-bounds Trim).
- **Rich node vocabulary** -- Source (file / network / synthetic), Sink
  (file / null / memory), 13 built-in filter types, Split, Merge, Null.

## Quick start

```rust
use oximedia_pipeline::builder::PipelineBuilder;
use oximedia_pipeline::node::{SourceConfig, SinkConfig};
use oximedia_pipeline::execution_plan::{ExecutionPlanner, estimate_resources};

// 1. Build a pipeline
let graph = PipelineBuilder::new()
    .source("input", SourceConfig::File("video.mkv".into()))
    .scale(1280, 720)
    .hflip()
    .sink("output", SinkConfig::File("out.mkv".into()))
    .build()
    .expect("pipeline should validate");

assert!(graph.node_count() >= 3);

// 2. Plan execution
let plan = ExecutionPlanner::plan(&graph).expect("valid DAG");
println!("stages: {}", plan.stage_count());

// 3. Estimate resources
let res = estimate_resources(&plan, &graph);
println!("peak memory: {} MB, threads: {}, gpu: {}",
    res.peak_memory_mb, res.cpu_threads, res.gpu_recommended);
```

## Modules

| Module | Purpose |
|--------|---------|
| **`node`** | Core types: `NodeId`, `PadId`, `StreamKind`, `FrameFormat`, `StreamSpec`, `SourceConfig`, `SinkConfig`, `FilterConfig`, `NodeType`, `NodeSpec`. Every node and pad is strongly typed. |
| **`graph`** | `PipelineGraph` -- a DAG of `NodeSpec` connected by `Edge`. Supports `add_node`, `connect`, `remove_node`, `validate`, `topological_sort`, `predecessors`/`successors`, and source/sink enumeration. |
| **`builder`** | `PipelineBuilder` + `NodeChain` -- fluent API that auto-connects nodes via default pads. Shorthand methods: `scale`, `crop`, `trim`, `volume`, `fps`, `hflip`, `vflip`. Call `.build()` to validate and obtain the graph. |
| **`execution_plan`** | `ExecutionPlanner` groups nodes into `ExecutionStage`s by longest-path levelling. `estimate_resources` returns peak memory, CPU thread count, and GPU recommendation. `PipelineOptimizer` provides `fuse_consecutive_scales`, `eliminate_identity`, and a combined `optimize` pass. |

## Architecture

```text
                PipelineBuilder (fluent DSL)
                        |
                        v
              PipelineGraph (DAG of NodeSpec + Edge)
               /        |        \
        validate   topological   connect
                     sort
                        |
                        v
              ExecutionPlanner (level assignment)
                        |
                        v
              ExecutionPlan (Vec<ExecutionStage>)
                        |
                        v
              estimate_resources -> ResourceEstimate
```

**Node lifecycle:**

1. Create nodes (`NodeSpec::source`, `NodeSpec::filter`, `NodeSpec::sink`).
2. Add them to a `PipelineGraph` and `connect` pads (or use `PipelineBuilder`).
3. Optionally run `PipelineOptimizer::optimize` to fuse and prune.
4. Call `ExecutionPlanner::plan` to obtain a staged execution order.
5. Query `estimate_resources` for memory / thread / GPU hints.

## Built-in filters

| Filter | Description | Cost |
|--------|-------------|------|
| `Scale` | Resize to exact resolution | 8 |
| `Crop` | Extract rectangular region | 5 |
| `Trim` | Cut to a time window (ms) | 4 |
| `Volume` | Adjust audio gain (dB) | 2 |
| `Fps` | Force constant frame rate | 4 |
| `Format` | Convert pixel/sample format | 3 |
| `Overlay` | Composite secondary stream | 12 |
| `Concat` | Join segments end-to-end | 10 |
| `Pad` | Pad canvas with black fill | 6 |
| `Hflip` | Horizontal mirror | 1 |
| `Vflip` | Vertical flip | 1 |
| `Transpose` | Rotate by 90-degree multiples | 2 |
| `Custom` | User-defined name + params | 15 |

Cost values are used by the optimiser to estimate computational weight.

## License

Copyright COOLJAPAN OU (Team Kitasan). Part of the
[OxiMedia](https://github.com/cool-japan/oximedia) framework.

Licensed under the terms specified in the workspace root.
