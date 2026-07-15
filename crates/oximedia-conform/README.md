# oximedia-conform

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional media conforming system for OxiMedia — timeline reconstruction from EDL, XML, and AAF formats.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Core Functionality

- **EDL Import** — CMX 3600, CMX 3400 format support
- **XML Import** — Final Cut Pro XML, Adobe Premiere Pro XML, DaVinci Resolve XML
- **AAF Import** — Avid Media Composer timelines
- **Media Conforming** — Timecode-based clip extraction with handle extension
- **Output Sequence Assembly** — Multi-format source support

### Advanced Features

- **Offline/Online Workflow** — Proxy media matching and relinking
- **Timeline Reconstruction** — Multi-track video/audio with nested sequence support
- **Media Database** — SQLite-based catalog (rusqlite + r2d2) with fingerprinting and search
- **Quality Control** — Comprehensive validation and verification
- **Batch Processing** — Parallel processing of multiple sessions via rayon

### Matching Strategies

- **Filename Matching** — Exact, fuzzy (strsim), and glob pattern-based
- **Timecode Matching** — Frame-accurate with drop-frame support
- **Content Matching** — File size, duration, and checksum verification (SHA-256, MD5, xxHash)

### Output Formats

- **Sequence Export** — MP4, Matroska, DPX/TIFF/PNG sequences
- **Project Export** — EDL, FCP XML, Premiere XML, AAF
- **Report Generation** — JSON, HTML, CSV, Markdown formats

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-conform = "0.2.0"
```

```rust
use oximedia_conform::{ConformSession, ConformConfig};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = ConformSession::new(
        "My Conform".to_string(),
        PathBuf::from("timeline.edl"),
        vec![PathBuf::from("/media/sources")],
        PathBuf::from("/output/conformed"),
        ConformConfig::default(),
    )?;

    // Run the complete conform workflow
    let report = session.run().await?;

    println!("Conformed {}/{} clips",
             report.stats.matched_count,
             report.stats.total_clips);

    Ok(())
}
```

## API Overview (62 source files, 712 public items)

**Core types:**
- `ConformSession` — Main conform session orchestrating the full workflow
- `ConformConfig` — Configuration for matching strategy, handles, output formats
- `ConformReport` — Detailed report of conform results and statistics

**Modules:**
- `edl` — EDL (CMX 3600/3400) parser and importer
- `xml` — FCP XML, Premiere XML, DaVinci Resolve XML importers
- `aaf` — AAF timeline importer
- `database` — SQLite media catalog with fingerprinting
- `match_strategy` — Filename, timecode, and content-based matching
- `timeline` — Multi-track timeline reconstruction
- `qc` — Quality control and validation
- `batch` — Parallel batch processing
- `report` — Report generation (JSON, HTML, CSV, Markdown)
- `export` — Output format writers

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
