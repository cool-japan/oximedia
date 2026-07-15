# oximedia-presets

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Advanced encoding preset library for OxiMedia. Provides 200+ professional encoding presets covering major platforms, broadcast standards, streaming protocols, and quality tiers, with auto-selection, validation, and import/export.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Platform Presets** - YouTube, Vimeo, Facebook, Instagram, TikTok, Twitter, LinkedIn
- **Broadcast Standards** - ATSC, DVB, ISDB presets
- **Streaming Protocols** - HLS, DASH, SmoothStreaming, RTMP, SRT ABR ladders
- **Archive Formats** - Lossless and mezzanine presets
- **Mobile Optimization** - iOS and Android specific presets
- **Quality Tiers** - Low, medium, high, and highest quality options
- **Lazy Loading** - `PresetLibrary::global()` cached singleton via `OnceLock`; per-category `LazyPresetCategory` loads on first access
- **Auto-selection** - `OptimalPresetSelector::select(criteria, library)` picks the best scored preset; falls back to smallest when no match
- **Text Search** - `InvertedIndex` AND-semantics tokenized search in `PresetRegistry`
- **Fuzzy Lookup** - Alias map and Levenshtein-based fuzzy name search in `PresetRegistry`
- **ABR Ladders** - `AbrLadder` / `AbrRung` (height, bitrate, preset)
- **Validation** - Verify preset correctness and compatibility
- **Import/Export** - Share presets via JSON
- **Preset Chains** - Compose multiple presets in sequence
- **Preset Versioning** - Track preset version history
- **Preset Diff** - Compare preset configurations
- **Preset Metadata** - Tags, categories, and descriptions
- **Preset Override** - Override individual preset parameters
- **Preset Resolver** - Resolve preset dependencies
- **Preset Scoring** - Score presets for quality and performance
- **Preset Benchmarking** - Benchmark preset encoding performance
- **Delivery Presets** - Delivery-specific presets (broadcast, web, mobile)
- **Ingest Presets** - Ingest workflow presets
- **Color Presets** - Color grading presets

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-presets = "0.2.0"
```

```rust
use oximedia_presets::{PresetLibrary, PresetCategory};
use oximedia_presets::{OptimalPresetSelector, SelectionCriteria};

// Load all built-in presets
let library = PresetLibrary::new();

// Find presets by category
let youtube_presets = library.find_by_category(
    PresetCategory::Platform("YouTube".to_string())
);

// Select optimal preset for a target bitrate
let criteria = SelectionCriteria::default();
let scored = OptimalPresetSelector::select(&criteria, &library);
```

## API Overview

**Core types:**
- `PresetLibrary` — Main preset repository with search, filtering, and `global()` singleton
- `PresetRegistry` — Registry with alias map, `InvertedIndex` AND-search, and fuzzy lookup
- `Preset` / `PresetMetadata` — Preset data with category, tags, and encoding config
- `PresetCategory` — Category enum: Platform, Broadcast, Streaming, Archive, Mobile, Web, Social, Quality, Codec
- `AbrLadder` / `AbrRung` — ABR ladder configuration (height, bitrate, preset)
- `OptimalPresetSelector` / `SelectionCriteria` / `ScoredPreset` — scored auto-selection
- `LazyPresetCategory` — opt-in lazy per-category loading via `OnceLock`
- `BitrateRange` — Bitrate range for preset matching

**Modules:**
- `archive` — Archive/lossless presets
- `broadcast` — Broadcast standard presets (ATSC, DVB, ISDB)
- `codec` — Codec-specific presets
- `color_preset` — Color grading presets
- `custom` — Custom preset management
- `delivery_preset` — Delivery workflow presets
- `export` — Preset export
- `import` — Preset import
- `ingest_preset` — Ingest workflow presets
- `library` — Preset library
- `mobile` — Mobile platform presets
- `platform` — Platform presets (YouTube, Vimeo, Facebook, etc.)
- `preset_benchmark` — Benchmark encoding presets
- `preset_chain` — Preset composition chains
- `preset_diff` — Preset comparison
- `preset_export`, `preset_import` — Import/export
- `preset_manager` — Preset lifecycle management
- `preset_metadata` — Preset metadata
- `preset_override` — Parameter overrides
- `preset_resolver` — Dependency resolution
- `preset_scoring` — Quality/performance scoring
- `preset_tags` — Tag management
- `preset_versioning` — Version history
- `quality` — Quality tier presets
- `social` — Social media presets
- `streaming` — Streaming protocol presets
- `validate`, `validation` — Validation
- `web` — Web delivery presets

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
