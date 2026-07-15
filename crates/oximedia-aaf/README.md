# oximedia-aaf

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Advanced Authoring Format (AAF) support for OxiMedia — SMPTE ST 377-1 compliant reading and writing for professional post-production workflows.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- Full SMPTE ST 377-1 (AAF Object Specification) support
- SMPTE ST 2001 (AAF Operational Patterns) support
- Microsoft Structured Storage (compound file) parsing
- Complete object model: Mobs, Segments, Components, Effects, Operation Groups
- Dictionary support with extensibility for class/property/type definitions
- Essence reference handling (embedded and external)
- Timeline and edit rate management
- Metadata preservation and export
- Conversion to OpenTimelineIO and EDL formats
- Read and write capability
- No unsafe code

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-aaf = "0.2.0"
```

```rust
use oximedia_aaf::{AafFile, AafReader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open an AAF file
    let mut reader = AafReader::open("timeline.aaf")?;
    let aaf = reader.read()?;

    // Access composition mobs
    for comp_mob in aaf.composition_mobs() {
        println!("Composition: {}", comp_mob.name());
        for track in comp_mob.tracks() {
            println!("  Track: {}", track.name);
        }
    }
    Ok(())
}
```

## API Overview

**Core types:**
- `AafReader` — Opens and parses AAF compound files
- `AafFile` — Top-level AAF file representation
- `CompositionMob`, `Track`, `SourceClip`, `Sequence` — Object model
- `EdlExporter`, `XmlExporter` — Export to EDL and OpenTimelineIO
- `Timeline`, `TimelineClip`, `TimelineTrack` — Timeline abstraction

**Modules (29 source files, 853 public items):**
- `structured_storage` — Microsoft Structured Storage compound file parser
- `dictionary` — AAF class/property/type definitions
- `essence`, `media_data`, `media_file_ref` — Media essence references
- `composition_mob`, `composition` — Composition mob types
- `descriptor`, `object_model` — AAF object model
- `effects`, `effect_def`, `operation_group`, `parameter` — Effect handling
- `mob_slot`, `source_clip`, `selector`, `scope` — Timeline building blocks
- `interchange` — Data interchange structures
- `property_value` — Property value handling
- `metadata` — Metadata structures
- `convert` — Format conversion utilities
- `aaf_export` — AAF export functionality
- `writer` — AAF file writer

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
