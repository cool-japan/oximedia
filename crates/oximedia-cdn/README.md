# oximedia-cdn

CDN edge management, cache invalidation, and origin failover for OxiMedia

[![Crates.io](https://img.shields.io/crates/v/oximedia-cdn.svg)](https://crates.io/crates/oximedia-cdn)
[![Documentation](https://docs.rs/oximedia-cdn/badge.svg)](https://docs.rs/oximedia-cdn)
[![License](https://img.shields.io/crates/l/oximedia-cdn.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- Edge node management with composite scoring (capacity, latency, error rate, feature support)
- Glob-based cache invalidation with priority queuing and per-node rate limiting
- Origin failover with four strategies: Priority, WeightedRoundRobin, ResponseTimeBased, LeastConnections
- Haversine geo-routing with distance-based latency modeling
- Prometheus text format metrics export using lock-free AtomicU64 counters
- Unified `CdnManager` orchestrator with RwLock/Mutex-guarded subsystems
- `#![forbid(unsafe_code)]` -- fully safe Rust

## Quick Start

```toml
[dependencies]
oximedia-cdn = "0.2.0"
```

```rust
use oximedia_cdn::{
    CdnConfig, CdnManager,
    EdgeNode, EdgeFeature,
    InvalidationScope, InvalidationPriority,
};

// Create a CDN manager with default configuration
let config = CdnConfig::default();
let manager = CdnManager::new(config);

// Register edge nodes with feature tags
let mut node = EdgeNode::new("edge-tokyo-01", "https://tok1.cdn.example.com");
node.add_feature(EdgeFeature::Hls);
node.add_feature(EdgeFeature::LowLatency);
node.set_capacity(10_000);
manager.add_edge_node(node);

// Select the best node for a request
if let Some(best) = manager.best_node_for(&["hls", "low-latency"]) {
    println!("Route to: {} (score: {:.2})", best.id(), best.score());
}

// Invalidate cached content by glob pattern
manager.invalidate(
    InvalidationScope::Glob("/live/stream-*.ts".into()),
    InvalidationPriority::High,
);
```

## Modules

### `edge_manager`

Manages a pool of CDN edge nodes and selects the best candidate for each request. Key types:

- `EdgeFeature` -- capability tags (e.g., HLS, DASH, LowLatency, DRM)
- `EdgeNode` -- edge server with capacity, latency, error count, and feature set; exposes a composite `score()` combining all health signals
- `EdgeManager` -- node pool with registration, removal, and selection
- `best_node_for()` -- selects the highest-scoring node matching required features
- `failover_chain()` -- returns ranked fallback candidates
- `overloaded_nodes()` -- identifies nodes exceeding capacity threshold

### `cache_invalidation`

Purges stale content from edge caches with scoped targeting and rate control. Key types:

- `InvalidationScope` -- `Url | PathPrefix | Glob | Tag | All`
- `glob_match()` -- pattern matcher supporting `*`, `**`, and `?` wildcards
- `InvalidationRequest` -- scoped purge with priority and timestamp
- `InvalidationResult` -- per-node success/failure tracking
- `InvalidationQueue` -- priority queue with per-node rate limiting to avoid thundering herd
- `InvalidationPriority` -- `Low | Normal | High | Critical`
- `InvalidationManager` -- legacy batch API for bulk operations

### `origin_failover`

Manages origin server pools with automatic health tracking and failover. Key types:

- `OriginStrategy` -- `Priority | WeightedRoundRobin | ResponseTimeBased | LeastConnections`
- `OriginServer` -- origin with atomic health counters, EWMA response time, and weight
- `OriginPool` -- ordered server collection with strategy-based selection
- `HealthChecker` -- periodic probing with configurable interval and failure threshold

### `geo_routing`

Geographic request routing using great-circle distance calculations. Key types:

- `Region` -- named geographic region (e.g., `Region::new("ap-northeast-1")`)
- `GeoLocation` -- latitude/longitude coordinate pair
- `haversine_km()` -- great-circle distance between two points
- `latency_from_km()` -- estimates network latency from geographic distance
- `EdgeNodeGeo` -- binds an edge node ID to a geographic location
- `GeoRouter` -- selects the nearest edge node for a client location

### `cdn_metrics`

Lock-free metrics collection with Prometheus-compatible text export. Key types:

- `CdnMetrics` -- global counters (requests, bytes, errors, cache hits/misses)
- `EdgeMetrics` -- per-node counters using AtomicU64 for contention-free updates
- `MetricSnapshot` / `EdgeSnapshot` -- point-in-time reads for reporting
- `MetricsRegistry` -- collects all node metrics and exports Prometheus text format

## Architecture

The crate is organized around five modules unified by the `CdnManager` orchestrator.
`EdgeManager` maintains the node pool, `GeoRouter` handles geographic selection, and
`OriginPool` provides failover to origin servers. Cache coherence is managed through
`InvalidationManager` with priority queuing and rate limiting. `MetricsRegistry` observes
the entire system and exports Prometheus-formatted counters.

All subsystems within `CdnManager` are guarded by `RwLock` or `Mutex` for safe concurrent
access. All modules share a common `CdnError` type. No unsafe code is used anywhere in the crate.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)
