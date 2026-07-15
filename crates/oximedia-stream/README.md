# oximedia-stream

Adaptive streaming pipeline, segment lifecycle management, and stream health monitoring for OxiMedia

[![Crates.io](https://img.shields.io/crates/v/oximedia-stream.svg)](https://crates.io/crates/oximedia-stream)
[![Documentation](https://docs.rs/oximedia-stream/badge.svg)](https://docs.rs/oximedia-stream)
[![License](https://img.shields.io/crates/l/oximedia-stream.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

## Features

- BOLA-inspired adaptive bitrate (ABR) pipeline with quality ladder management
- Segment lifecycle state machine with prefetch scheduling and eviction policies
- Quality-of-Experience (QoE) health scoring with issue detection
- SCTE-35 splice information binary encoding, decoding, and PTS-based scheduling (full roundtrip coverage: SpliceInsert, TimeSignal, multi-descriptor, byte-identical, truncated, zero-descriptor)
- Multi-CDN failover routing with EWMA latency tracking and pluggable strategies
- HLS master/media playlist and DASH MPD manifest generation (pure string-based, no XML crate)
- Media segment packaging with keyframe-aligned splitting and file output
- Zero-copy CMAF scatter-gather: `CmafChunk.data` is `bytes::Bytes`; `write_cmaf_segment` returns `Vec<Bytes>` for efficient scatter-gather I/O
- `#![forbid(unsafe_code)]` -- fully safe Rust

## Quick Start

```toml
[dependencies]
oximedia-stream = "0.2.0"
```

```rust
use oximedia_stream::{
    AdaptivePipeline, QualityLadder, SwitchReason,
    SegmentManager, StreamHealthMonitor,
    build_master_playlist, StreamVariant,
};

// Create an adaptive pipeline with the default 6-tier quality ladder (240p-4K)
let ladder = QualityLadder::default_ladder();
let mut pipeline = AdaptivePipeline::new(ladder);

// Feed bandwidth samples from completed segment downloads
pipeline.record_download(500_000, 1.0); // 500 KB in 1 second
pipeline.update_buffer(15.0);           // 15 seconds of buffer

// Evaluate whether a quality switch is warranted
if let Some(switch) = pipeline.evaluate_switch() {
    println!("Switched to tier {} ({:?})", switch.to_tier, switch.reason);
}
```

## Modules

### `adaptive_pipeline`

BOLA-inspired ABR algorithm that selects quality tiers based on measured bandwidth and buffer level. Key types:

- `QualityTier` -- resolution, bitrate, codec, and minimum bandwidth for a single rendition
- `QualityLadder` -- ordered collection of tiers with a `default_ladder()` covering 240p to 4K
- `BandwidthEstimator` -- sliding-window EWMA estimator with percentile support
- `AdaptivePipeline` -- main controller with buffer-stress, bandwidth-driven, and recovery switching
- `QualitySwitch` / `SwitchReason` -- switch event records

### `segment_manager`

Tracks every media segment from creation through download, playback, and eviction. Key types:

- `SegmentState` -- `Pending | Downloading | Available | Evicted | Failed`
- `MediaSegment` -- full metadata including PTS, duration, size, tier, and download speed
- `SegmentManager` -- pool with configurable buffer depth, prefetch count, and eviction policy

### `stream_health`

QoE scoring and issue detection. Computes a composite 0-100 health score from buffer depth (40%), bitrate (30%), rebuffer rate (20%), and switch frequency (10%). Key types:

- `HealthIssue` -- `BufferTooLow`, `FrequentSwitches`, `HighRebufferRate`, `LowBitrate`, `HighLatency`, `DroppedFramesExcessive`
- `StreamHealthReport` -- point-in-time snapshot with score and detected issues
- `StreamHealthMonitor` -- accumulates events and maintains a history ring-buffer

### `scte35`

SCTE-35 splice information encoding and parsing per SCTE 35-2019. All binary codec work is byte-by-byte with no external serialization crate. Key types:

- `SpliceInfoSection` -- top-level section with protocol version, tier, and command
- `SpliceInsert` / `TimeSignal` -- splice command payloads
- `SpliceDescriptor` -- `AvailDescriptor` and `SegmentationDescriptor` with encode/parse round-trip
- `BreakDuration` -- 33-bit duration in 90 kHz ticks
- `SpliceScheduler` -- PTS-based min-heap scheduler for timed event dispatch
- `encode_splice_insert()` / `parse_splice_info()` -- public codec API

### `multi_cdn`

Multi-CDN failover routing with four strategies. Latency tracked via EWMA (alpha = 0.2); consecutive failures mark providers unavailable. Key types:

- `CdnProvider` -- edge node with atomic latency, error count, and availability
- `RoutingStrategy` -- `Primary | RoundRobin | LatencyBased | WeightedRandom`
- `FailoverPolicy` -- configurable error threshold and timeout
- `MultiCdnRouter` -- selects providers, records latency/errors/successes

### `manifest_builder`

Pure-Rust HLS and DASH manifest generation via string concatenation. Key types and functions:

- `build_master_playlist()` -- HLS master with BANDWIDTH, RESOLUTION, CODECS, FRAME-RATE
- `build_media_playlist()` -- HLS media with EXTINF, BYTERANGE, DISCONTINUITY, PROGRAM-DATE-TIME
- `build_dash_mpd()` -- DASH MPD XML with Representation, BaseURL, and SegmentTemplate
- `ManifestFormat` -- `HlsV3 | HlsV6 | HlsV7 | DashMpd`
- `StreamVariant` / `HlsManifest` / `DashMpd` / `DashRepresentation` / `SegmentTemplate`

### `stream_packager`

Accumulates `MediaUnit`s and flushes them into `PackagedSegment`s on keyframe-aligned boundaries. Key types:

- `MediaUnit` -- single encoded NAL unit or audio frame with PTS/DTS and keyframe flag
- `PackagedSegment` -- finalized segment with concatenated data and duration
- `SegmentPackager` -- stateful packager with configurable segment/target duration and CMAF mode
- `SegmentWriter` trait / `FileSegmentWriter` -- output sink abstraction

## Architecture

The crate is organized around seven independent modules that compose into a full streaming pipeline. The `AdaptivePipeline` drives quality decisions, feeding into the `SegmentManager` for segment lifecycle tracking. The `StreamHealthMonitor` observes the pipeline and produces QoE reports. SCTE-35 support enables ad insertion workflows. The `MultiCdnRouter` handles origin selection, while `manifest_builder` and `stream_packager` produce the final HLS/DASH output.

All modules share a common `StreamError` type. No unsafe code is used anywhere in the crate.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)

Version: 0.2.0 — 2026-07-15 — extensively tested
