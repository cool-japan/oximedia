# oximedia-ndi

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

NDI (Network Device Interface) support for OxiMedia. A clean-room implementation of the NDI protocol that doesn't rely on the official NDI SDK, providing mDNS-based discovery, low-latency streaming, tally lights, and PTZ control.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **mDNS-based source discovery** - Automatic NDI source discovery on the local network
- **Low-latency video streaming** - Sub-frame latency with Full HD and 4K support
- **Audio/video synchronization** - Frame-accurate A/V sync
- **Tally light support** - Program/preview indicators for live production
- **PTZ control** - Pan-Tilt-Zoom camera control commands
- **Bandwidth adaptation** - Dynamic quality adjustment based on network conditions
- **SpeedHQ codec** - Intra-frame codec for efficient NDI transport
- **Failover support** - Automatic source failover
- **Source groups** - Group-based source filtering
- **Genlock** - Genlock synchronization support
- **Frame synchronization** - Frame sync buffer management
- **Source registry** - Local source registry and management
- **Connection state tracking** - Connection lifecycle management
- **Statistics** - Streaming statistics and performance metrics
- **Clock synchronization** - Network clock synchronization
- **Routing** - NDI source routing

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-ndi = "0.2.0"
```

```rust
use oximedia_ndi::{NdiSource, NdiSender, SenderConfig};
use std::time::Duration;

// Discover NDI sources on the network
let sources = NdiSource::discover_sources(Duration::from_secs(5)).await?;
for source in &sources {
    println!("Found: {} at {}", source.name(), source.address());
}

// Create an NDI sender
let sender = NdiSender::new(SenderConfig::default()).await?;
```

## API Overview

**Core types:**
- `NdiSource` — Represents a discoverable NDI source with connect/disconnect methods
- `NdiSender` / `SenderConfig` — Send video/audio frames as an NDI source
- `NdiReceiver` / `ReceiverConfig` — Receive frames from an NDI source
- `DiscoveryService` / `NdiSourceInfo` — mDNS-based source discovery
- `TallyServer` / `TallyState` — Tally light server and state
- `PtzCommand` — Pan/Tilt/Zoom control commands
- `VideoFormat` / `AudioFormat` — Frame format descriptors
- `NdiConfig` — Global NDI configuration
- `SpeedHqCodec` / `YuvFormat` — Codec support

**Modules (public):**
- `audio_config`, `audio_format` — Audio configuration and formats
- `av_buffer` — Audio/video buffer management
- `bandwidth` — Bandwidth estimation and adaptation
- `channel_map` — Audio channel mapping
- `clock_sync` — Clock synchronization
- `connection_config`, `connection_state` — Connection management
- `failover` — Source failover handling
- `frame_buffer`, `frame_sync` — Frame buffering and synchronization
- `genlock` — Genlock synchronization
- `group` — Source group management
- `metadata`, `metadata_frame` — Metadata frame support
- `ndi_stats` — Streaming statistics
- `ptz` — PTZ control
- `quality` — Quality metrics
- `routing` — Source routing
- `sender_config` — Sender configuration
- `source_filter`, `source_registry` — Source filtering and registry
- `statistics` — Performance statistics
- `stream_info` — Stream information
- `tally_bus`, `tally_manager` — Tally management
- `transport` — Transport layer
- `video_format` — Video format definitions

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
