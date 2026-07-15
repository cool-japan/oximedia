# oximedia-clips

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional clip management and logging system for OxiMedia, providing database-backed clip organization, metadata, subclip creation, and EDL export.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Clip Database** — SQLite-backed persistent storage with async I/O via Pure-Rust OxiSQL (`oxisql-sqlite-compat`)
- **Subclip Creation** — Create subclips with precise in/out points
- **Clip Grouping** — Bins, folders, and collections organization
- **Professional Logging** — Keywords, markers, ratings, and notes
- **Take Management** — Track multiple takes of the same shot
- **Proxy Association** — Link clips to proxy versions
- **Smart Collections** — Auto-updating collections based on criteria
- **Search and Filter** — Advanced full-text and field-based search
- **Import/Export** — EDL, XML, CSV, JSON export
- **Audit Trail** — Clip history and change tracking
- **Storyboard** — Visual storyboard from clip selection
- **Timeline Integration** — Clip timeline view

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-clips = "0.2.0"
```

```rust
use oximedia_clips::{ClipManager, Clip, Rating};
use std::path::PathBuf;

async fn example() -> Result<(), Box<dyn std::error::Error>> {
    let manager = ClipManager::new(":memory:").await?;

    let mut clip = Clip::new(PathBuf::from("/path/to/video.mov"));
    clip.set_name("Interview Take 1");
    clip.set_rating(Rating::FourStars);
    clip.add_keyword("interview");

    let clip_id = manager.add_clip(clip).await?;
    let results = manager.search("interview").await?;
    Ok(())
}
```

## API Overview (68 source files, 787 public items)

**Core types:**
- `ClipManager` — Main async clip management entry point
- `Clip`, `ClipId`, `SubClip` — Clip representation
- `Bin`, `Folder`, `Collection`, `SmartCollection` — Organization types
- `Marker`, `MarkerType` — Timeline markers
- `Rating`, `Keyword`, `Favorite` — Logging metadata
- `Note`, `Annotation` — Clip notes
- `ProxyLink`, `ProxyQuality` — Proxy management
- `Take`, `TakeSelector` — Take management

**Modules:**
- `database` — SQLite persistence layer (OxiSQL-based async, Pure Rust)
- `search` — Full-text and field search
- `export` — EDL/XML/CSV/JSON export
- `clip_timeline` — Timeline view of clips
- `storyboard` — Visual storyboard generation
- `clip_audit` — Audit trail and change history

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
