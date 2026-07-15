# oximedia-graph

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 692](https://img.shields.io/badge/tests-692-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

Filter graph pipeline for OxiMedia, providing a directed acyclic graph (DAG) implementation for processing media through composable filter pipelines.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Overview

`oximedia-graph` provides a DAG-based media processing pipeline:

- **Nodes**: Processing units that transform media data
- **Ports**: Connection points for data flow
- **Frames**: Data units passed through the graph
- **Context**: Runtime state and statistics

## Architecture

```
┌─────────┐     ┌─────────┐     ┌─────────┐
│ Source  │────▶│ Filter  │────▶│  Sink   │
│ (Decode)│     │ (Scale) │     │(Encode) │
└─────────┘     └─────────┘     └─────────┘
```

## Features

- **Video Filters** — Scale, Crop, Pad, ColorConvert, FPS, Deinterlace, Overlay, Delogo, Denoise, Grading, IVTC, LUT, Timecode, Tonemap
- **Audio Filters** — Resample, ChannelMix, Volume, Normalize, Equalizer, Compressor, Limiter, Delay
- **Graph Optimization** — Merge compatible nodes, dead node elimination
- **Topological Sort** — Automatic execution order via topological sort
- **Cycle Detection** — Detect and report cycles in the graph
- **Graph Merging** — Merge sub-graphs into a unified pipeline
- **Graph Partitioning** — Partition graphs for distributed execution
- **Visualization** — Graph structure visualization
- **Serialization** — Graph save/load
- **Metrics** — Per-node processing statistics
- **Profiling** — Pipeline execution profiling
- **Subgraph** — Named subgraph support
- **Node Cache** — Cached node results for repeated frames
- **Validation** — Graph structural validation

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-graph = "0.2.0"
```

```rust
use oximedia_graph::graph::GraphBuilder;
use oximedia_graph::filters::video::{PassthroughFilter, NullSink};
use oximedia_graph::node::NodeId;
use oximedia_graph::port::PortId;

// Create a simple graph: source -> sink
let source = PassthroughFilter::new_source(NodeId(0), "source");
let sink = NullSink::new(NodeId(0), "sink");

let (builder, source_id) = GraphBuilder::new().add_node(Box::new(source));
let (builder, sink_id) = builder.add_node(Box::new(sink));

let graph = builder
    .connect(source_id, PortId(0), sink_id, PortId(0))
    .unwrap()
    .build()
    .unwrap();

assert_eq!(graph.node_count(), 2);
```

```rust
use oximedia_graph::context::GraphContext;

let mut context = GraphContext::new();
graph.process(&mut context, frame)?;

let stats = context.stats();
println!("Frames processed: {}", stats.frames_processed);
```

## API Overview

**Core types:**
- `FilterGraph` / `GraphBuilder` — Graph construction and execution
- `Node` — Processing node trait
- `NodeId`, `PortId` — Node and port identifiers
- `FilterFrame`, `FrameRef` — Frame data
- `GraphContext`, `ProcessingStats` — Runtime context and statistics
- `GraphError` — Error type

**Modules:**
- `graph` — FilterGraph and GraphBuilder
- `node`, `node_registry`, `node_cache` — Node management
- `port` — Port types and connections
- `frame` — Frame types and pool
- `context` — Runtime context
- `error` — Error types
- `data_flow` — Data flow tracking
- `edge_weight` — Edge weight/priority
- `scheduler` — Node execution scheduling
- `optimization` — Graph optimization passes
- `topological` — Topological sort
- `cycle_detect` — Cycle detection
- `graph_merge` — Graph merging
- `graph_validation` — Graph structural validation
- `graph_stats` — Graph statistics
- `pipeline_graph` — Pipeline-specific graph
- `processing_graph` — Processing graph abstraction
- `subgraph` — Named sub-graph
- `dependency_graph` — Dependency graph
- `graph_partition` — Graph partitioning
- `layout` — Graph layout for visualization
- `visualization` — Visual graph rendering
- `serialize` — Graph serialization
- `metrics_graph` — Metrics collection
- `profiling` — Execution profiling
- `filters` — Filter implementations (video and audio)

**Video filters (`filters::video`):**
- `scale`, `crop`, `pad` — Geometry
- `color` — Color conversion
- `fps`, `deinterlace` — Temporal
- `overlay` — Compositing
- `delogo` — Logo removal
- `denoise` — Denoising
- `grading` — Color grading
- `ivtc` — Inverse telecine
- `lut` — LUT application
- `timecode` — Timecode overlay
- `tonemap` — HDR tone mapping

**Audio filters (`filters::audio`):**
- `resample`, `channel_mix`, `volume`, `normalize`, `equalizer`, `compressor`, `limiter`, `delay`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
