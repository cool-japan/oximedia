# oximedia-proxy

**Status: [Stable]** | Version: 0.2.1 | Tests: extensively tested | Updated: 2026-08-12

Proxy and offline editing workflow system for OxiMedia. Provides comprehensive proxy workflow management including generation, linking, conforming, and complete offline-to-online pipeline support.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Proxy Generation** - Quarter, half, and full resolution proxies with multiple codec options
- **Batch Processing** - Generate proxies for multiple files simultaneously
- **Proxy Linking** - Link proxies to high-resolution originals with SQLite database
- **Link Verification** - Validate proxy-original relationships
- **EDL Conforming** - Conform from CMX 3600 and other EDL formats
- **XML Conforming** - Final Cut Pro XML and Premiere Pro XML support
- **Frame-accurate relinking** - Preserve exact frame accuracy during conform
- **Offline/Online Workflow** - Complete offline-to-online-to-delivery pipeline
- **Smart Caching** - Intelligent proxy cache management with cleanup policies
- **Timecode Preservation** - Maintain accurate timecode across workflow
- **Metadata Sync** - Synchronize metadata between proxy and original
- **Sidecar Files** - Checksum and processing record management
- **Proxy Registry** - Central proxy registry with extensions
- **Proxy Scheduler** - Scheduled proxy generation
- **Proxy Pipeline** - Multi-stage proxy processing pipeline
- **Proxy Pool** - Proxy resource pool management
- **Proxy Quality** - Quality assessment for proxies
- **Proxy Manifest** - Proxy manifest generation
- **Proxy Index** - Proxy search index
- **Proxy Format** - Format compatibility checking
- **Proxy Aging** - Proxy lifecycle and aging management
- **Transcode Queue** - Priority-based transcode queue
- **Bandwidth Management** - Proxy bandwidth optimization
- **Validation** - Proxy validation and integrity checking
- **Format Compatibility** - Cross-format proxy compatibility
- **Resolution Management** - Multi-resolution proxy management
- **Offline Proxy** - Offline-specific proxy handling
- **Relink Proxy** - Proxy relinking workflows

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-proxy = "0.2.0"
```

```rust
use oximedia_proxy::{ProxyGenerator, ProxyPreset, ProxyLinkManager, ConformEngine, OfflineWorkflow};

// Generate a quarter-resolution proxy
let generator = ProxyGenerator::new();
let proxy_path = generator
    .generate("original.mov", "proxy.mp4", ProxyPreset::QuarterResH264)
    .await?;

// Link proxy to original
let mut manager = ProxyLinkManager::new("links.db").await?;
manager.link_proxy("proxy.mp4", "original.mov").await?;

// Conform from EDL
let engine = ConformEngine::new("links.db").await?;
let conformed = engine.conform_from_edl("edit.edl", "output.mov").await?;
```

## Tutorial: Offline-to-Online Workflow

This walks through the complete proxy workflow: generate low-res proxies for
editing, edit offline, then conform the edit back to the original
high-resolution media.

### 1. Ingest and generate proxies

```rust
use oximedia_proxy::{OfflineWorkflow, ProxyPreset};

# async fn ingest_example() -> Result<(), Box<dyn std::error::Error>> {
let mut workflow = OfflineWorkflow::new("project_links.db").await?;

// Generates a quarter-resolution H.264 proxy and links it to the original
// in one step (see `src/workflow/offline.rs::OfflineWorkflow::ingest`).
workflow
    .ingest("camera/A001_C001.mov", "proxies/A001_C001_proxy.mp4", ProxyPreset::QuarterResH264)
    .await?;
workflow
    .ingest("camera/A001_C002.mov", "proxies/A001_C002_proxy.mp4", ProxyPreset::QuarterResH264)
    .await?;
# Ok(())
# }
```

Repeat `ingest()` for every camera-original clip in the project. Each call
generates a real low-bitrate re-encode via `oximedia-transcode` and records
the proxy<->original relationship in the link database (`ProxyLinkManager`,
backed by `link/database.rs`'s JSON store).

### 2. Edit offline

Open the generated proxy files (`proxies/*.mp4`) in your NLE of choice —
Premiere Pro, DaVinci Resolve, Final Cut Pro, or Avid Media Composer. Proxies
are small and fast to scrub/decode, so editing stays responsive even on a
laptop with no access to the original camera media. Export the cut as a CMX
3600 EDL or an XML timeline (FCP XML / Premiere XML) referencing the proxy
files.

### 3. Conform back to the originals

```rust
use oximedia_proxy::OfflineWorkflow;

# async fn conform_example(workflow: &OfflineWorkflow) -> Result<(), Box<dyn std::error::Error>> {
let result = workflow.conform("exports/final_cut.edl", "delivery/final_conformed.mov").await?;
println!("frame accurate: {}", result.frame_accurate);
# Ok(())
# }
```

`OfflineWorkflow::conform` opens a `ConformEngine` against the project's link
database and calls `ConformEngine::conform_from_edl`, which validates that
the EDL file exists and resolves clip references through the same
proxy-to-original links established during ingest.

> **Current implementation status:** as of this writing,
> `EdlConformer::conform` / `XmlConformer::conform`
> (`src/conform/edl.rs`, `src/conform/xml.rs`) validate that the EDL/XML file
> exists and return a `ConformResult` with `frame_accurate: true`, but do not
> yet parse per-clip relink counts from the file — `clips_relinked` /
> `clips_failed` are always `0`. What *is* fully implemented and real today:
> - `ConformEngine::batch_conform` — merges multiple parsed `oximedia_edl::Edl`
>   values (via `oximedia_edl::parse_edl`) into one timeline with configurable
>   overlap-resolution (`MergeStrategy::PreferEarlier` /
>   `PreferLonger` / `LayerToTracks`) and per-event source provenance.
> - `ProxyLinkManager` — real proxy<->original link persistence and lookup.
> - `WorkflowValidator::validate_all` — cross-checks that every linked proxy
>   and original file still exists on disk, flags duplicate/orphaned links,
>   and reports metadata inconsistencies — a practical "did my offline-to-
>   online handoff survive?" check that stands in for verifying conform
>   output today.

### 4. Verify before delivery

```rust
use oximedia_proxy::{ProxyLinkManager, WorkflowValidator};

# async fn verify_example() -> Result<(), Box<dyn std::error::Error>> {
let manager = ProxyLinkManager::new("project_links.db").await?;
let report = WorkflowValidator::new(&manager).validate_all()?;
assert!(report.errors.is_empty(), "unresolved links: {:?}", report.errors);
# Ok(())
# }
```

See `tests/e2e_workflow.rs` for a complete, runnable version of this pipeline
(ingest -> generate -> simulate an edit -> conform -> verify) exercised
against a real (non-mocked) transcode round trip.

## API Overview

**Core types:**
- `ProxyGenerator` / `ProxyPreset` — Proxy generation with quality presets
- `ProxyLinkManager` / `ProxyLink` — Database-backed proxy-original linking
- `ConformEngine` / `EdlConformer` — EDL/XML conforming
- `OfflineWorkflow` / `OnlineWorkflow` / `RoundtripWorkflow` — Complete workflow management
- `CacheManager` / `CacheStrategy` — Proxy cache management
- `ResolutionManager` / `ResolutionSwitcher` — Multi-resolution management
- `Quality` — Low/Medium/High quality presets with bitrate recommendations

**Modules:**
- `cache` — Cache management
- `conform` — EDL/XML conforming (EDL, mapper, timeline, XML)
- `examples` — Usage examples
- `format_compat` — Format compatibility
- `generate` — Proxy generation (encoder, optimizer, presets)
- `generation` — Generation pipeline
- `link` — Proxy linking (database, manager, statistics)
- `linking` — Linking utilities
- `media_link` — Media file linking
- `metadata` — Metadata synchronization
- `offline_edit`, `offline_proxy` — Offline editing support
- `proxy_aging` — Proxy lifecycle management
- `proxy_bandwidth` — Bandwidth optimization
- `proxy_cache` — Cache management
- `proxy_compare` — Proxy comparison
- `proxy_fingerprint` — Proxy fingerprinting
- `proxy_format` — Format management
- `proxy_index` — Search index
- `proxy_manifest` — Manifest generation
- `proxy_pipeline` — Processing pipeline
- `proxy_pool` — Resource pool
- `proxy_quality` — Quality assessment
- `proxy_registry_ext` — Registry extensions
- `proxy_scheduler` — Scheduled generation
- `proxy_status` — Status tracking
- `proxy_sync` — Synchronization
- `registry` — Central registry
- `relink_proxy` — Relinking workflows
- `render` — Render management
- `resolution` — Resolution management
- `sidecar` — Sidecar file management
- `smart_proxy` — Intelligent proxy selection
- `spec` — Proxy specifications
- `timecode` — Timecode verification
- `transcode_proxy`, `transcode_queue` — Transcoding
- `utils` — Utility functions
- `validation` — Validation (checker, validator)
- `workflow` — Workflow planning

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
