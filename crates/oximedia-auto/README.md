# oximedia-auto

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Automated video editing for OxiMedia, providing intelligent highlight detection, smart cutting, auto-assembly, and rules-based editing.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Highlight Detection** — Motion intensity, face tracking, audio peak detection, multi-factor scoring
- **Smart Cutting** — Shot boundary detection, beat-synced cuts, dialogue-aware cutting, jump cut removal
- **Auto-Assembly** — Highlight reels, trailers, social media clips (15s/30s/60s), automatic pacing
- **Rules Engine** — Shot duration constraints, transition preferences, music synchronization, aspect ratio adaptation
- **Scene Scoring** — Multi-feature content classification, sentiment analysis, interest curve generation, auto-titling
- **Async Architecture** — Tokio-based async processing for large video collections

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-auto = "0.2.0"
```

```rust
use oximedia_auto::{AutoEditor, AutoEditorConfig};
use oximedia_auto::assembly::AssemblyType;
use oximedia_auto::rules::PacingPreset;

// Create an auto editor for highlight reels
let config = AutoEditorConfig::default()
    .with_assembly_type(AssemblyType::HighlightReel)
    .with_target_duration_ms(60_000)  // 60 seconds
    .with_pacing(PacingPreset::Fast);

let editor = AutoEditor::new(config);
```

```rust
use oximedia_auto::{AutoEditor, AutoEditorConfig};
use oximedia_auto::assembly::AssemblyType;
use oximedia_auto::rules::AspectRatio;

// Configure for vertical social media (9:16)
let config = AutoEditorConfig::default()
    .with_assembly_type(AssemblyType::SocialClip)
    .with_target_duration_ms(30_000)
    .with_aspect_ratio(AspectRatio::Vertical9x16);
```

## API Overview (28 source files, 492 public items)

**Core types:**
- `AutoEditor` — Main automated editing engine
- `AutoEditorConfig` — Editor configuration

**Modules:**
- `highlights` — Highlight moment detection (motion intensity, face tracking, audio peaks)
- `cuts` — Intelligent cut point selection (shot boundaries, beat sync, dialogue-aware)
- `assembly` — Final edit assembly (highlight reel, trailer, social clip variants)
- `rules` — Editing rules and pacing presets (duration constraints, transitions, aspect ratios)
- `scoring` — Scene importance scoring, content classification, interest curve generation

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
