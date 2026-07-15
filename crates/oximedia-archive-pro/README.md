# oximedia-archive-pro

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional archive and digital preservation system for OxiMedia. Provides comprehensive tools for long-term digital preservation including BagIt/OAIS packaging, format migration, multi-algorithm checksums, PREMIS/METS metadata, fixity checking, and risk assessment.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Preservation Packaging** — BagIt, OAIS (SIP/AIP/DIP), TAR, and ZIP packaging
- **Format Migration** — Planning, execution, and validation of format migrations
- **Checksum Management** — Multi-algorithm checksums (MD5, SHA-256, SHA-512, xxHash/xxh3, BLAKE3)
- **Metadata Preservation** — PREMIS, METS, Dublin Core metadata support
- **Version Control** — Track versions and changes over time
- **Fixity Checking** — Periodic scheduled integrity verification
- **Risk Assessment** — Format obsolescence monitoring and scoring
- **Emulation Support** — Prepare for future emulation needs
- **Documentation** — Auto-generate preservation documentation
- **Cold Storage** — Cold/deep archive management
- **Disaster Recovery** — Recovery planning and procedures
- **Retention Policies** — Configurable retention schedules
- **Recommended Formats** — FFV1/MKV, FLAC, WAV, TIFF, PNG, JPEG2000, PDF/A

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-archive-pro = "0.2.0"
```

```rust
use oximedia_archive_pro::{
    package::bagit::BagItBuilder,
    checksum::{ChecksumAlgorithm, ChecksumGenerator},
    metadata::premis::PremisMetadata,
};

// Create a BagIt package
let bag = BagItBuilder::new(std::path::PathBuf::from("/path/to/bag"))
    .with_algorithm(ChecksumAlgorithm::Sha256)
    .with_metadata("Contact-Name", "Archivist")
    .add_file(std::path::Path::new("/path/to/media.mkv"))?
    .build()?;
```

## API Overview (68 source files, 792 public items)

**Core types:**
- `PreservationFormat` — Recommended preservation formats enum
- `BagItBuilder` — Fluent BagIt package builder
- `ChecksumGenerator` — Multi-algorithm checksum computation

**Modules:**
- `package` — BagIt, OAIS (SIP/AIP/DIP), TAR, and ZIP packaging
- `checksum` — Multi-algorithm checksum management and verification
- `metadata` — PREMIS, METS, and Dublin Core metadata
- `fixity` — Periodic fixity checking and scheduling
- `format_migration` — Migration planning, execution, and validation
- `risk` — Format obsolescence risk assessment and scoring
- `version` — Version control and change tracking
- `emulation` — Emulation environment preparation
- `documentation` — Auto-generated preservation documentation
- `cold_storage` — Cold/deep archive management
- `disaster_recovery` — Recovery planning and procedures
- `retention` — Configurable retention policy management

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
