# oximedia-routing

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional audio/video routing and patching system for OxiMedia.  Provides full any-to-any routing via crosspoint matrices, virtual patch bays, complex channel mapping, SDI audio embedding, ST 2110 IP media routing, MADI support, NMOS IS-04/05/07/08/09/11 REST APIs, and timecode-driven routing automation.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 | Tests: extensively tested — 2026-07-14

## Features

- **Crosspoint Matrix** - Full any-to-any audio routing matrix
- **Virtual Patch Bay** - Input/output management with flexible patching
- **Channel Mapping** - Complex channel remapping (e.g., 5.1 to stereo downmix)
- **Signal Flow Graph** - Signal flow visualization and validation
- **Audio Embedding** - Audio embedding/de-embedding for SDI
- **Format Conversion** - Sample rate, bit depth, and channel count conversion
- **Gain Staging** - Per-channel gain control with metering
- **Monitoring** - AFL/PFL/Solo monitoring systems
- **Preset Management** - Save/load routing configurations
- **MADI Support** - 64-channel MADI routing
- **Dante Integration** - Dante audio-over-IP metadata support
- **NMOS IS-04/05/07/08/09/11** - Network media open specifications (see below)
- **Automation** - Time-based routing changes with timecode (24/25/29.97/30/50/59.94/60 fps)
- **IP Routing** - ST 2110 IP media routing
- **Failover** - Automatic failover routing
- **Route Optimization** - Dijkstra-based policy-driven route selection (`ZeroLatencyOptimizer`)
- **Bandwidth Budgeting** - Network bandwidth management for IP routes
- **Latency Calculation** - End-to-end latency budgeting
- **Topology Mapping** - Network topology visualization
- **Redundancy Groups** - Managed redundancy for critical routes
- **Traffic Shaping** - QoS and traffic shaping for media flows
- **ValidateCache** - Topology-version-aware signal-flow validation cache
- **Lock-free Router** - Glitch-free real-time routing updates
- **Mix-minus** - Broadcast IFB mix-minus routing
- **Tally System** - Active-path tally signalling

## Cargo features

| Feature | What it enables |
|---------|-----------------|
| `nmos-http` | NMOS REST API server (`NmosHttpServer`, `hyper` + `tokio` + `serde_json`) |
| `nmos-discovery` | mDNS/DNS-SD registry discovery (implies `nmos-http`, adds `mdns-sd`) |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-routing = "0.2.0"

# Enable NMOS HTTP server + mDNS discovery
# oximedia-routing = { version = "0.1.9", features = ["nmos-discovery"] }
```

```rust
use oximedia_routing::prelude::*;

// Create a 16x8 crosspoint matrix
let mut matrix = CrosspointMatrix::new(16, 8);
matrix.connect(0, 0, Some(-6.0)).unwrap(); // Input 0 → Output 0 at -6 dB

// Create a patch bay
let mut bay = PatchBay::new();
let input = bay.input_manager_mut()
    .add_input("Mic 1".to_string(), SourceType::Microphone);

// 5.1 to stereo downmix
let remapper = ChannelRemapper::downmix_51_to_stereo();
```

## NMOS APIs

All AMWA NMOS specifications use versioned REST paths.  Enable the `nmos-http`
feature to activate the HTTP server.

| Specification | Version | Base path |
|---------------|---------|-----------|
| IS-04 Node API | v1.3 | `/x-nmos/node/v1.3/` |
| IS-05 Connection API | v1.1 | `/x-nmos/connection/v1.1/` |
| IS-07 Events API | v1.0 | `/x-nmos/events/v1.0/` |
| IS-08 Channel Mapping API | v1.0 | `/x-nmos/channelmapping/v1.0/` |
| IS-09 System API | v1.0 | `/x-nmos/system/v1.0/` |
| IS-11 Stream Compatibility | — | `/x-nmos/streamcompatibility/` |

Resources registered in `NmosRegistry`: nodes, devices, sources, flows, senders,
receivers.  Supported `NmosFormat`: Video, Audio, Data, Mux.  Supported
`NmosTransport`: RtpMulticast, RtpUnicast, Dash, Hls, Srt.

IS-07 (`nmos::is07::Is07EventSource`) emits JSON events
`{id, event_type, sequence, value}` with a monotonically increasing sequence
number for dropped-event detection.

### mDNS / DNS-SD discovery

Enable the `nmos-discovery` feature.  `NmosDiscovery` browses three service
types via the `mdns-sd` crate:

- `_nmos-node._tcp.local.`
- `_nmos-query._tcp.local.`
- `_nmos-registration._tcp.local.`

Browse timeout: 500 ms.  `NmosRegistryInfo` carries name, host, port, and
priority from the TXT `pri` record.

## ValidateCache — topology-version caching

`validate_cache::ValidateCache` wraps `SignalFlowGraph` with a `u64` version
counter.  Calling `validate()` returns the cached `ValidationResult` when the
topology version is unchanged, and recomputes only when a mutation has bumped
the counter.

Mutating methods that increment the version: `add_input()`, `add_output()`,
`connect()`, `remove_node()`, `remove_edge()`.

```rust
use oximedia_routing::validate_cache::ValidateCache;

let mut vc = ValidateCache::new();
vc.add_input("mic".to_string());
vc.add_output("bus".to_string());
vc.connect("mic".to_string(), "bus".to_string());

let result = vc.validate(); // computes and caches
let result2 = vc.validate(); // returns cached — no recompute
```

## ZeroLatencyOptimizer — Dijkstra path selection

`zero_latency::ZeroLatencyOptimizer` finds the minimum-latency path between two
nodes in the signal graph using Dijkstra's algorithm (min-heap
`BinaryHeap<QueueEntry>` with reverse ordering).

Edge weights are expressed in **samples** so the optimizer is sample-rate aware.
`find_lowest_latency(src, dst)` returns `Option<MonitorPath>`.

Configuration:
- `avoid_categories` — set of node categories to skip (e.g., high-latency DSP)
- `max_latency_samples` — hard latency budget; 0 = unlimited

## Sub-frame timeline — supported frame rates

`automation::timeline::FrameRate` covers all broadcast and film timecode
standards:

| Variant | Rate | Notes |
|---------|------|-------|
| `Fps24` | 24 fps | Film |
| `Fps25` | 25 fps | PAL |
| `Fps2997Df` | 29.97 fps drop-frame | NTSC broadcast |
| `Fps2997Ndf` | 29.97 fps non-drop | NTSC non-drop |
| `Fps30` | 30 fps | HD broadcast |
| `Fps50` | 50 fps | 50 Hz high-frame |
| `Fps5994` | 59.94 fps | High-frame NTSC |
| `Fps60` | 60 fps | High-frame |

`AutomationTimeline` schedules routing changes at sample-accurate `Timecode`
positions.  `Timecode::from_frames(total_frames, frame_rate)` converts a frame
count to hours/minutes/seconds/frames using the selected `FrameRate`.

## API Overview

- `CrosspointMatrix` — Any-to-any routing matrix with gain per crosspoint
- `PatchBay` — Virtual patch bay with input/output management
- `ChannelRemapper` / `ChannelLayout` — Channel mapping and downmix
- `SignalFlowGraph` — DAG-based signal flow with validation
- `ValidateCache` — Version-tracked cached validation of `SignalFlowGraph`
- `GainStage` / `MultiChannelGainStage` — Per-channel gain control
- `SoloManager` / `AflMonitor` / `PflMonitor` — Monitoring systems
- `MadiInterface` — 64-channel MADI support
- `PresetManager` / `RoutingPreset` — Configuration presets
- `AutomationTimeline` — Timecode-based routing automation
- `ZeroLatencyOptimizer` — Dijkstra minimum-latency path optimizer
- `NmosHttpServer` — NMOS REST API server (feature `nmos-http`)
- `NmosDiscovery` — mDNS/DNS-SD registry browser (feature `nmos-discovery`)
- Modules: `matrix`, `patch`, `channel`, `flow`, `embed`, `convert`, `gain`, `monitor`, `preset`, `madi`, `dante`, `nmos`, `automation`, `matrix_router`, `signal_path`, `ip_router`, `path_selector`, `crosspoint_matrix`, `failover_route`, `route_table`, `signal_monitor`, `routing_policy`, `bandwidth_budget`, `route_optimizer`, `link_aggregation`, `latency_calc`, `route_preset`, `route_audit`, `topology_map`, `redundancy_group`, `traffic_shaper`, `validate_cache`, `zero_latency`, `lock_free_router`, `mix_minus`, `tally_system`, `aes67`, `gpio_trigger`, `bulk_ops`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
