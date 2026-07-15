# oximedia-packager

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Adaptive streaming packaging (HLS/DASH) for OxiMedia. Provides comprehensive support for packaging media content into HLS and DASH adaptive streaming formats with encryption, cloud upload, and live/VOD support.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **HLS packaging** - Master playlists, media playlists, TS and fMP4 segments, M3U8 manifests
- **DASH packaging** - MPD manifests, CMAF segments, multi-period support
- **Automatic bitrate ladder** - Generate multi-bitrate variant streams automatically
- **Segment formats** - TS, fMP4, CMAF segment creation
- **Encryption** - AES-128, SAMPLE-AES, CENC content protection
- **Keyframe alignment** - Ensure segments start on keyframes
- **Low latency mode** - LL-HLS and LL-DASH support
- **Cloud upload** - S3/cloud storage integration (optional feature)
- **Live and VOD** - Both live streaming and video-on-demand workflows
- **Manifest versioning** - Track manifest versions
- **Bandwidth estimation** - Accurate bandwidth estimation for variant selection
- **Bitrate calculation** - Segment and stream bitrate calculation
- **DRM information** - DRM info and encryption info management
- **Subtitle tracks** - Subtitle track packaging
- **Timed metadata** - Timed metadata (ID3/SCTE-35) insertion
- **Segment indexing** - Segment index and list management
- **Manifest building** - Flexible manifest builder API
- **Manifest updates** - Live manifest incremental updates

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-packager = "0.2.0"

# Optional features
# oximedia-packager = { version = "0.1.9", features = ["encryption", "s3"] }
```

```rust
use oximedia_packager::{Packager, PackagerConfig, PackagerBuilder, PackagingFormat};

// Package to HLS
let packager = PackagerBuilder::new()
    .format(PackagingFormat::HlsFmp4)
    .low_latency(true)
    .build()?;

packager.package_hls("input.mkv", "output/hls").await?;
packager.package_dash("input.mkv", "output/dash").await?;
```

## Feature Flags

- `hls` (default) — HLS packaging support
- `dash` (default) — DASH packaging support
- `encryption` — AES-128 / CENC content encryption
- `s3` — AWS S3 cloud upload integration

## API Overview

**Core types:**
- `Packager` / `PackagerBuilder` — Main packaging engine with fluent builder API
- `HlsPackager` / `HlsPackagerBuilder` — HLS-specific packager
- `DashPackager` / `DashPackagerBuilder` — DASH-specific packager
- `PackagerConfig` — Full packaging configuration
- `BitrateLadder` / `BitrateEntry` — Bitrate ladder definition
- `SegmentConfig` / `SegmentFormat` — Segment duration and format settings
- `EncryptionConfig` / `EncryptionMethod` — Content protection settings
- `LadderGenerator` — Automatic bitrate ladder generation

**Modules:**
- `bandwidth_estimator` — Bandwidth estimation
- `bitrate_calc` — Bitrate calculation
- `cmaf` — CMAF segment packaging
- `config` — Packaging configuration
- `dash` — DASH packaging (MPD, packager)
- `drm_info` — DRM information
- `encryption`, `encryption_info` — Encryption support
- `error` — Error types
- `hls` — HLS packaging (packager, playlist)
- `ladder` — Bitrate ladder
- `low_latency` — Low-latency packaging (LL-HLS/LL-DASH)
- `manifest`, `manifest_builder`, `manifest_update` — Manifest generation
- `multivariant` — Multivariant playlist
- `output` — Output handling
- `packaging_config` — Packaging configuration
- `playlist_generator` — Playlist generation
- `segment`, `segment_index`, `segment_list`, `segment_validator` — Segment management
- `subtitle_track` — Subtitle track packaging
- `timed_metadata` — Timed metadata insertion

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
