# oximedia-cache

[![Crates.io](https://img.shields.io/crates/v/oximedia-cache.svg)](https://crates.io/crates/oximedia-cache)
[![Documentation](https://docs.rs/oximedia-cache/badge.svg)](https://docs.rs/oximedia-cache)
[![License](https://img.shields.io/crates/l/oximedia-cache.svg)](LICENSE)

High-performance caching infrastructure for [OxiMedia](https://github.com/cool-japan/oximedia) --
the Sovereign Media Framework.

Version: 0.2.1 — 2026-08-12 — extensively tested

`oximedia-cache` provides seven complementary caching primitives designed for
multimedia workloads: from single-node O(1) LRU caches to distributed
consistent-hash clusters with quorum replication.

## Features

- **100% Pure Rust** -- no C/Fortran dependencies, no `unsafe`
- **Zero external runtime dependencies** -- only `thiserror` for error types
- **Arena-backed O(1) LRU** -- insert, lookup, and eviction in constant amortised time
- **Multi-tier caching** -- L1/L2/disk-sim with pluggable eviction per tier and automatic promotion
- **Predictive cache warming** -- EMA inter-arrival smoothing, autocorrelation periodicity detection, score-ranked warmup plans
- **Bloom filters** -- standard and counting (4-bit saturating) variants with FNV-1a double hashing (Kirsch-Mitzenmacher construction)
- **Consistent hashing** -- virtual-node hash ring for stable key routing, `get_n_nodes` for replica selection
- **Six eviction policies** -- LRU, LFU, FIFO, Random, TinyLFU (Count-Min admission gate), ARC (adaptive ghost-list tracking)
- **Content-aware caching** -- media-type priority scoring, per-type TTL, recency x priority x size eviction metric

## Quick Start

### LRU Cache

```rust
use oximedia_cache::lru_cache::LruCache;

let mut cache: LruCache<&str, Vec<u8>> = LruCache::new(128);
cache.insert("frame-001", vec![0u8; 4096], 4096);
assert!(cache.get(&"frame-001").is_some());

let stats = cache.stats();
println!("hits={} misses={} evictions={}", stats.hits, stats.misses, stats.evictions);
```

### Multi-Tier Cache

```rust
use oximedia_cache::tiered_cache::{TieredCache, TierConfig, EvictionPolicy};

let cache = TieredCache::new(vec![
    TierConfig {
        name: "L1".into(),
        capacity_bytes: 64 * 1024,
        access_latency_us: 1,
        eviction_policy: EvictionPolicy::Lru,
    },
    TierConfig {
        name: "L2".into(),
        capacity_bytes: 1024 * 1024,
        access_latency_us: 10,
        eviction_policy: EvictionPolicy::Lfu,
    },
]);
```

### Bloom Filter

```rust
use oximedia_cache::bloom_filter::BloomFilter;

let mut bf = BloomFilter::new(10_000, 0.01); // 1% false-positive rate
bf.insert(b"segment-42");
assert!(bf.contains(b"segment-42"));   // definitely present
assert!(!bf.contains(b"segment-99"));  // probably absent
```

## Modules

### `lru_cache`

Arena-backed doubly-linked-list LRU cache. All nodes live in a `Vec<Option<LruNode>>`
with free-list recycling, avoiding per-node heap allocations. Tracks hit/miss/eviction
counters and cumulative `size_bytes`. Key API: `new`, `get`, `insert`, `remove`, `peek`,
`evict_lru`, `stats`.

### `tiered_cache`

Multi-tier cache (`TieredCache`) where each tier has independent `TierConfig` (name,
capacity in bytes, simulated latency, eviction policy). Reads cascade through tiers in
order; a hit in a lower tier promotes the entry to L1. Supports `warmup` for cold-start
bulk loading and `invalidate` for cross-tier removal. Eviction policies per tier:
`Lru`, `Lfu`, `Fifo`, `Random`, `TinyLfu`.

### `cache_warming`

Predictive warming via `CacheWarmer`. Records per-key `AccessPattern` histories, computes
`frequency_per_hour`, predicts next access via exponential inter-arrival EMA (alpha=0.3),
and detects periodic patterns using normalised autocorrelation. `plan_warmup` produces a
budget-constrained `WarmupPlan` ranked by `frequency x recency_weight x size_efficiency`.

### `bloom_filter`

Probabilistic membership filters using FNV-1a double hashing. `BloomFilter` provides
classic bit-array membership with optimal `m` and `k` computed from expected item count
and target false-positive rate. `CountingBloomFilter` extends this with 4-bit saturating
nibble counters, enabling safe deletion via `remove`.

### `distributed_cache`

Consistent-hash ring (`ConsistentHash`) with virtual nodes for stable key routing as
nodes join and leave. `DistributedCacheClient` provides per-node routing, `ReplicationFactor`
models quorum read/write logic (including RF-3 presets), and `CacheCoordinator` ties
clients together with `can_read_quorum` / `can_write_quorum` checks.

### `eviction_policies`

Standalone eviction data structures decoupled from any cache backend:
- `FrequencyCounter` -- windowed frequency estimator with exponential decay (halving)
- `LfuEvictionTracker` -- O(1) amortised LFU via frequency buckets with FIFO tie-breaking
- `TinyLfuAdmission` -- two-phase admission gate (doorkeeper Bloom + counting Bloom + frequency counter)
- `ArcTracker` -- Adaptive Replacement Cache ghost-list tracker (T1/T2/B1/B2) with self-tuning parameter `p`

### `content_aware_cache`

Media-type-aware cache (`ContentAwareCache`) layered on top of `LruCache`. Entries are
scored for eviction by `(1 - recency) x (1 / priority) x size_factor` rather than
pure LRU order. Six `MediaContentType` variants (VideoSegment, AudioSegment, Image,
Manifest, Thumbnail, Metadata) each with assigned priority (3-10) and recommended TTL
(30s to 24h). Supports optional byte-level capacity via `with_max_bytes` and periodic
TTL expiry sweeps via `evict_expired`.

## Part of OxiMedia

This crate is a workspace member of [OxiMedia](https://github.com/cool-japan/oximedia),
the patent-free, memory-safe multimedia processing framework written in pure Rust.

## License

Copyright COOLJAPAN OU (Team Kitasan). Licensed under the terms specified in the
workspace root.
