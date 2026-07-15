# oximedia-metadata

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Comprehensive metadata standards support for OxiMedia, parsing and writing all major media metadata formats including ID3v2, Vorbis Comments, APEv2, iTunes, XMP, EXIF, IPTC, QuickTime, and Matroska.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **ID3v2** (v2.3, v2.4) — MP3 tag metadata
- **Vorbis Comments** — Ogg/FLAC/Opus metadata
- **APEv2** — APE and other lossless format tags
- **iTunes metadata** — MP4/M4A metadata atoms
- **XMP** — Adobe Extensible Metadata Platform (XML-based)
- **EXIF** — Image metadata (JPEG, TIFF)
- **IPTC** — Photo metadata (legacy IIM and modern)
- **QuickTime metadata** — MOV user data atoms
- **Matroska tags** — MKV/WebM tag format
- Unicode support (UTF-8, UTF-16)
- Picture/artwork handling with embedded images
- Custom tags and user-defined fields
- Format conversion with cross-format field mapping
- Metadata validation and sanitization
- Character encoding detection/conversion via encoding_rs
- Multiple value support per field
- Hierarchical metadata (XMP namespaces)
- Metadata diff and merge
- Sidecar file support (XMP sidecar)
- Rights metadata
- Linked data (schema.org)
- Provenance tracking
- Schema registry
- Bulk update operations
- Template-based metadata

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-metadata = "0.2.0"
```

```rust
use oximedia_metadata::{Metadata, MetadataFormat, MetadataValue};

let mut metadata = Metadata::new(MetadataFormat::Id3v2);

metadata.insert("TIT2".to_string(), MetadataValue::Text("My Song".to_string()));
metadata.insert("TPE1".to_string(), MetadataValue::Text("Artist Name".to_string()));

if let Some(MetadataValue::Text(title)) = metadata.get("TIT2") {
    println!("Title: {}", title);
}
```

## API Overview

**Core types:**
- `Metadata` — Metadata container
- `MetadataFormat` — Format enum (Id3v2, VorbisComment, Apev2, iTunes, Xmp, Exif, Iptc, etc.)
- `MetadataValue` — Value types (Text, Binary, Integer, Picture, etc.)
- `CommonFields` — Format-agnostic common field access
- `MetadataConverter` — Cross-format conversion
- `MetadataEmbed` — Embed metadata into media files; `embed()` does real container-aware splicing for ID3v2 (prepend), APEv2 (append), and Exif/XMP (JPEG `APP1` segment, or bare-blob merge) — Matroska, IPTC, Vorbis Comments, iTunes, and QuickTime targets return `Error::Unsupported` rather than a corrupting byte concatenation

**Format modules:**
- `id3v2` — ID3v2 tag parser/writer
- `vorbis` — Vorbis Comment parser/writer
- `apev2` — APEv2 tag parser/writer
- `itunes` — iTunes MP4 atoms
- `xmp` — XMP (XML) parser/writer
- `exif`, `exif_parse` — EXIF data
- `iptc`, `iptc_iim` — IPTC metadata
- `quicktime` — QuickTime user data
- `matroska` — Matroska tags

**Utility modules:**
- `converter` — Format conversion
- `metadata_merge`, `metadata_diff` — Merge and diff
- `metadata_sanitize` — Sanitization
- `metadata_template` — Template system
- `metadata_history` — History tracking
- `metadata_index` — Indexing support
- `metadata_stats` — Statistics
- `sidecar` — XMP sidecar files
- `schema`, `schema_registry` — Metadata schema
- `rights_metadata` — Rights information
- `linked_data` — schema.org linked data
- `provenance` — Provenance tracking
- `bulk_update` — Bulk operations
- `embedding` — Embedding utilities
- `tag_normalize` — Tag normalization
- `field_validator` — Field validation
- `metadata_export` — Export operations
- `search` — Metadata search

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
