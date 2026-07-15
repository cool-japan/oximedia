# oximedia-edl

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

CMX 3600 Edit Decision List (EDL) parser and generator for OxiMedia, with comprehensive support for broadcast EDL formats.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **EDL Formats** — CMX 3600, CMX 3400, GVG, Sony BVE-9000
- **Event Types** — Cut, Dissolve, Wipe, Key
- **Timecode Support** — Drop-frame and non-drop-frame at 24, 25, 30, 60 fps
- **Reel Names** — Source reel reference tracking with reel registry and mapping
- **Motion Effects** — Speed changes, reverse playback, freeze frames
- **Audio Channel Mapping** — Multi-channel audio routing
- **EDL Validation** — Compliance and conformance checking
- **Format Conversion** — Convert between EDL formats
- **EDL Merging** — Merge multiple EDLs with conflict resolution
- **Batch Export** — Batch processing of multiple EDLs
- **Conform Report** — Conformance report generation
- **EDL Comparison** — Diff two EDLs
- **EDL Consolidation** — Consolidate EDL events
- **EDL Filtering** — Filter events by criteria
- **EDL Statistics** — Statistical analysis of edit decisions
- **EDL Timeline** — Timeline representation of edit events
- **EDL Comments** — Comment handling and preservation
- **Transition Events** — Extended transition metadata
- **Frame Count** — Frame count utilities
- **Roundtrip** — Parse-serialize roundtrip verification
- **Optimizer** — EDL optimization and cleanup

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-edl = "0.2.0"
```

```rust
use oximedia_edl::{parse_edl, Edl};

let edl_text = r#"
TITLE: Example EDL
FCM: DROP FRAME

001  AX       V     C        01:00:00:00 01:00:05:00 01:00:00:00 01:00:05:00
* FROM CLIP NAME: shot001.mov
"#;

let edl = parse_edl(edl_text)?;
assert_eq!(edl.title, Some("Example EDL".to_string()));
```

```rust
use oximedia_edl::{Edl, EdlFormat, EdlGenerator};

let mut edl = Edl::new(EdlFormat::Cmx3600);
edl.set_title("My EDL".to_string());
edl.set_frame_rate(oximedia_edl::timecode::EdlFrameRate::Fps25);

let generator = EdlGenerator::new();
let output = generator.generate(&edl)?;
```

## API Overview

**Core types:**
- `parse_edl()` — Parse EDL text into structured representation
- `Edl` — Edit Decision List structure
- `EdlFormat` — Format variant (CMX3600, CMX3400, GVG, BVE9000)
- `EdlGenerator` — Generate EDL text output
- `EdlValidator` — Validate EDL conformance
- `EdlError` — Error type

**Modules:**
- `cmx3600` — CMX 3600 format parser/generator
- `parser` — Generic EDL parser
- `generator` — EDL text generator
- `event`, `event_list` — Edit event types and lists
- `transition_events` — Transition event metadata
- `timecode` — Timecode representation and arithmetic
- `frame_count` — Frame count utilities
- `audio` — Audio channel mapping
- `motion` — Motion effects (speed, reverse, freeze)
- `reel`, `reel_map`, `reel_registry` — Reel management
- `edl_event` — Extended event types
- `edl_filter` — Event filtering
- `edl_merge` — EDL merging
- `edl_compare` — EDL comparison / diff
- `edl_statistics` — Statistical analysis
- `edl_timeline` — Timeline representation
- `edl_comments` — Comment handling
- `edl_validator` — Validation logic
- `batch_export` — Batch processing
- `conform_report` — Conformance reporting
- `consolidate` — EDL consolidation
- `converter` — Format conversion
- `validator` — Conformance checking
- `metadata` — EDL metadata
- `optimizer` — EDL optimization
- `roundtrip` — Roundtrip verification
- `error` — Error types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
