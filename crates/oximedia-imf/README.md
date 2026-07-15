# oximedia-imf

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 512](https://img.shields.io/badge/tests-512-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

IMF (Interoperable Master Format) support for OxiMedia, providing SMPTE ST 2067-compliant package creation, validation, and parsing for professional broadcast and streaming delivery.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **CPL** — Composition Playlist parsing and generation (SMPTE ST 2067-3)
- **PKL** — Packing List with SHA-1/MD5 checksums (SMPTE ST 429-8)
- **ASSETMAP** — Asset map file handling (SMPTE ST 429-9)
- **OPL** — Output Profile List (SMPTE ST 2067-8)
- **MXF Essence** — Video/audio/subtitle track file handling
- **Full SMPTE conformance** — ST 2067-2, -3, -5, -8; ST 429-8, -9
- **Hash verification** — SHA-1, MD5 essence hash checking
- **Timeline validation** — Composition timeline structural validation
- **Supplemental packages** — Support for supplemental IMF packages
- **Package versioning** — Package version management
- **HDR metadata** — HDR metadata support in compositions
- **Multi-channel audio** — Audio layout per SMPTE ST 2067-8
- **Subtitles and captions** — IMSC1 subtitle resource handling
- **Markers and annotations** — Marker resource and annotation support
- **Multiple compositions** — Multiple CPLs per package
- **Application profile compliance** — IMP application profile validation
- **Delivery** — Delivery manifest and package construction
- **IMF Report** — Package validation and analysis reporting
- **XML utilities** — IMF XML namespace and utility functions
- **Content version** — Content version identification
- **Essence constraints** — Essence parameter constraint checking

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-imf = "0.2.0"
```

```rust
use oximedia_imf::{ImfPackage, ImfError};

// Load an IMF package from a directory
let package = ImfPackage::open("/path/to/imp")?;
let cpl = package.primary_cpl()
    .ok_or_else(|| ImfError::MissingElement("CompositionPlaylist".into()))?;
println!("Title: {}", cpl.content_title());
println!("Duration: {} frames", cpl.total_duration());
package.validate()?;
```

```rust
use oximedia_imf::{ImfPackageBuilder, EditRate};

let builder = ImfPackageBuilder::new("/path/to/output")
    .with_title("My IMF Package")
    .with_creator("OxiMedia")
    .with_edit_rate(EditRate::new(24, 1));

builder.add_video_track("/path/to/video.mxf")?;
builder.add_audio_track("/path/to/audio.mxf")?;
let package = builder.build()?;
```

## API Overview

**Core types:**
- `ImfPackage` — IMF package container
- `ImfPackageBuilder` — Package construction builder
- `EditRate` — Editorial frame rate (numerator/denominator)
- `ImfError` — Error type

**Package structure modules:**
- `asset_map`, `assetmap` — Asset map handling (SMPTE ST 429-9)
- `composition_sequence` — Composition sequence
- `composition_map` — Composition mapping
- `content_version` — Content version identification
- `application_profile` — Application profile compliance

**CPL modules:**
- `cpl_parser` — CPL XML parser
- `cpl_segment` — CPL segment handling
- `cpl_validator` — CPL validation
- `cpl_merge` — CPL merging
- `cpl` — CPL data structures (private API)

**PKL modules:**
- `pkl_document` — PKL document
- `pkl` — PKL parsing (private API)

**OPL modules:**
- `output_profile_list` — OPL document
- `opl_document` — OPL data
- `opl` — OPL parsing (private API)

**Essence modules:**
- `essence_descriptor` — Essence parameter descriptors
- `essence_hash` — Essence hash verification
- `essence_constraints` — Essence constraint validation
- `mxf_descriptor` — MXF descriptor types
- `track_file` — Track file reference
- `essence` — Essence data structures (private API)

**Resource modules:**
- `imsc1` — IMSC1 subtitle resource
- `subtitle_resource` — Subtitle resource handling
- `marker_list` — Marker list
- `marker_resource` — Marker resource
- `audio_layout` — Multi-channel audio layout

**Package validation and delivery:**
- `package_validator` — Package conformance validation
- `validator` — General validation (private API)
- `package` — Package types (private API)
- `supplemental_package` — Supplemental package support
- `delivery` — Delivery manifest
- `sidecar` — Sidecar file handling
- `versioning` — Package versioning

**Utilities:**
- `imf_timeline` — IMF timeline representation
- `imf_report` — Package analysis report
- `xml_util` — XML namespace and parsing utilities

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
