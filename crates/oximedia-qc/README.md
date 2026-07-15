# oximedia-qc

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Quality control and validation for OxiMedia. Provides comprehensive quality control and validation for media files ensuring they meet technical specifications and delivery requirements.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Video Quality Checks** - Codec validation, resolution, frame rate, bitrate analysis, interlacing detection, black/freeze frame detection, compression artifacts
- **Audio Quality Checks** - Codec validation, sample rate, loudness compliance (EBU R128, ATSC A/85), clipping detection, silence detection, phase issues, DC offset detection
- **Container Checks** - Format validation, stream synchronization, timestamp continuity, keyframe interval, seeking capability, duration consistency
- **Compliance Checks** - Broadcast delivery specs, streaming platform requirements (YouTube, Vimeo), custom rule sets
- **Report Generation** - JSON, XML, PDF, and database report export
- **Custom Rules** - Define and apply custom validation thresholds
- **Statistical Analysis** - Detailed quality metrics and statistics
- **Batch Processing** - Process multiple files in parallel
- **Scheduler** - Scheduled QC job execution
- **Temporal QC** - Frame-by-frame temporal quality analysis
- **HDR QC** - HDR-specific quality checks
- **Bitrate QC** - Bitrate compliance and consistency checks
- **File QC** - File-level integrity checks
- **Profile Management** - Named QC profiles and custom profiles
- **Detector Library** - Library of specialized quality detectors
- **Database Storage** - Persistent QC results storage

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-qc = "0.2.0"
```

```rust
use oximedia_qc::{QualityControl, QcPreset};

let qc = QualityControl::with_preset(QcPreset::Streaming);
let report = qc.validate("video.mkv")?;

if report.overall_passed {
    println!("All checks passed!");
} else {
    for error in report.errors() {
        println!("Error: {}", error.message);
    }
}
```

## Feature Flags

- `json` (default) — JSON report export
- `xml` (default) — XML report export
- `database` (default) — SQLite results database
- `pdf` (default) — PDF report generation

## API Overview

**Core types:**
- `QualityControl` — Main QC engine with preset and custom rule support
- `QcPreset` — Streaming, Broadcast, Archive, Custom presets
- `QcReport` — Detailed validation report with errors, warnings, and statistics

**Modules:**
- `audio` — Audio quality checks (loudness, clipping, silence, DC offset, phase)
- `batch` — Batch QC processing
- `bitrate_qc` — Bitrate compliance checks
- `compliance` — Broadcast and streaming platform compliance
- `database` — QC results database
- `detectors` — Specialized quality detectors
- `file_qc` — File-level integrity checks
- `hdr_qc` — HDR quality checks
- `lib` — Core QC engine
- `profiles`, `qc_profile` — QC profile management
- `qc_scheduler` — Scheduled QC jobs
- `temporal_qc` — Temporal quality analysis
- `utils` — Utility functions
- `video_measure` — Video measurement and metrics

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
