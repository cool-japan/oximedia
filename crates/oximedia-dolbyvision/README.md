# oximedia-dolbyvision

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Dolby Vision RPU (Reference Processing Unit) metadata parser and writer for OxiMedia. Provides metadata-only support, respecting Dolby's intellectual property.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **RPU Parsing and Writing** — Parse and generate Dolby Vision RPU metadata bitstreams
- **Profile Support** — Dolby Vision profiles 5, 7, 8, 8.1, 8.4
- **Content Metadata Levels** — Levels 1–11 (brightness, color, mastering display, etc.)
- **Profile Conversion** — Convert between compatible Dolby Vision profiles
- **Scene Processing** — Scene boundary detection and scene trim passes
- **Shot Metadata** — Shot-level and frame-level metadata extraction
- **Tone Mapping** — Tone mapping curve and enhancement layer representation
- **Mastering Display** — Mastering display metadata with primaries and luminance
- **Target Display** — Target display configuration and capabilities
- **Ambient Metadata** — Ambient viewing environment metadata
- **XML Export** — XML metadata export for delivery workflows
- **Delivery Specification** — Delivery spec validation for streaming/broadcast
- **Display Configuration** — Display configuration management
- **Frame Analysis** — Per-frame and level analysis
- **Validation** — Conformance validation for Dolby Vision compliance
- **Optional serde** — Feature-gated serde serialization/deserialization

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-dolbyvision = "0.2.0"
# With serde support:
oximedia-dolbyvision = { version = "0.2.0", features = ["serde"] }
```

```rust
use oximedia_dolbyvision::{DolbyVisionRpu, Profile};

// Create new RPU for Profile 8.4
let rpu = DolbyVisionRpu::new(Profile::Profile8_4);
assert_eq!(rpu.profile, Profile::Profile8_4);
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `serde` | Serde serialization/deserialization for all metadata types |

## API Overview

**Core types:**
- `DolbyVisionRpu` — RPU metadata container
- `Profile` — Dolby Vision profile enum (5, 7, 8, 8.1, 8.4)
- `DolbyVisionError` — Error type
- `Level1Metadata`–`Level11Metadata` — Content metadata levels
- `TrimPass`, `MetadataBlock` — RPU structure types
- `TonemapParams`, `ReshapingLut`, `ColorVolumeLut` — Tone mapping

**Modules:**
- `parser` — RPU bitstream parser (private API)
- `writer` — RPU bitstream writer (private API)
- `rpu` — Core RPU data structures (private API)
- `profiles` — Profile-specific processing
- `profile_convert` — Profile conversion between compatible profiles
- `tone_mapping` — Tone mapping operations
- `tonemap` — Internal tone map implementation (private API)
- `mastering` — Mastering display metadata
- `target_display` — Target display configuration
- `display_config` — Display configuration management
- `scene_trim` — Scene trim pass handling
- `shot_boundary` — Shot boundary detection
- `shot_metadata` — Shot-level metadata
- `frame_analysis` — Per-frame analysis
- `level_analysis` — Level-specific metadata analysis
- `level_mapping` — Level mapping between profiles
- `mapping_curve` — Tone mapping curve representation
- `metadata_block` — Metadata block types
- `dm_metadata` — Display management metadata
- `cm_analysis` — Color management analysis
- `enhancement` — Enhancement layer
- `ambient_metadata` — Ambient viewing metadata
- `compat` — Compatibility helpers
- `delivery_spec` — Delivery specification validation
- `validation` — Conformance validation
- `xml_metadata` — XML metadata export
- `trim_passes` — Trim pass processing

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
