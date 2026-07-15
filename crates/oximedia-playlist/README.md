# oximedia-playlist

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Broadcast playlist and scheduling system for OxiMedia. Provides comprehensive broadcast automation including frame-accurate timing, scheduling, secondary events, live integration, failover, SCTE-35 markers, and EPG generation.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Playlist Management** - Create and manage broadcast playlists with frame-accurate timing
- **Scheduling Engine** - Time-based playback scheduling with calendar and recurrence support
- **Automation** - Automated playout with pre-roll, post-roll, and event triggers
- **Secondary Events** - Graphics overlays, station logos, and scrolling tickers
- **Transitions** - Smooth transitions with audio/video crossfades
- **Live Integration** - Insert live content seamlessly into scheduled playlists
- **Failover** - Automatic backup content and filler management
- **Clock Sync** - Synchronization to wall clock or external timecode
- **Commercial Breaks** - SCTE-35 marker generation and break management
- **EPG Generation** - Electronic Program Guide with XMLTV export
- **As-run Logs** - Metadata tracking and as-run log generation
- **Multi-channel** - Support for multiple simultaneous broadcast channels
- **M3U playlists** - M3U/M3U8 playlist support
- **Smart Playback** - Smart playlist features (mood, energy-based ordering)
- **Shuffle** - Advanced shuffle algorithms
- **Recommendation** - Playlist recommendation engine
- **Track Ordering** - Intelligent track ordering
- **Playlist Health** - Health monitoring and validation
- **Crossfade** - Crossfade transition management
- **Repeat Policy** - Repeat and loop policies
- **Gap Filler** - Automatic gap detection and filler insertion
- **Interstitial** - Interstitial content insertion
- **Continuity** - Playlist continuity checking

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-playlist = "0.2.0"
```

```rust
use oximedia_playlist::playlist::{Playlist, PlaylistItem, PlaylistType};
use oximedia_playlist::schedule::ScheduleEngine;
use std::time::Duration;

// Create a new playlist
let mut playlist = Playlist::new("prime_time", PlaylistType::Linear);

// Add items to the playlist
let item = PlaylistItem::new("show_001.mxf")
    .with_duration(Duration::from_secs(3600));
playlist.add_item(item);

// Create a scheduling engine
let engine = ScheduleEngine::new();
```

## API Overview

**Core types:**
- `Playlist` / `PlaylistItem` / `PlaylistType` — Core playlist data model
- `ScheduleEngine` — Time-based scheduling engine
- `PlayoutEngine` — Automated playout controller
- `ClockSync` — Wall clock and timecode synchronization

**Modules:**
- `automation` — Automated playout control
- `backup` — Backup content and failover
- `clock` — Clock synchronization and offset
- `commercial` — Commercial break management
- `continuity` — Continuity checking
- `crossfade`, `crossfade_playlist` — Crossfade transitions
- `duration_calc` — Duration calculation
- `epg` — EPG and XMLTV generation
- `gap_filler` — Gap detection and filling
- `history`, `play_history` — Play history tracking
- `interstitial` — Interstitial content
- `live` — Live content insertion
- `metadata` — Playlist metadata (as-run, track)
- `multichannel` — Multi-channel playlist management
- `playlist` — Core playlist model
- `playlist_diff`, `playlist_export`, `playlist_filter` — Playlist utilities
- `playlist_health` — Health monitoring
- `playlist_merge` — Playlist merging
- `playlist_priority` — Priority-based playlist management
- `playlist_rules` — Business rules engine
- `playlist_segment` — Segment-based playlists
- `playlist_stats` — Statistics
- `playlist_sync` — Playlist synchronization
- `playlist_tempo`, `playlist_validator` — Tempo and validation
- `queue_manager` — Queue management
- `recommendation_engine` — Recommendation system
- `repeat_policy` — Repeat/loop policies
- `schedule` — Scheduling engine with recurrence
- `secondary` — Secondary events (graphics, tickers)
- `shuffle` — Shuffle algorithms
- `smart_play` — Smart playback
- `track_metadata`, `track_order` — Track management
- `transition` — Transition effects
- `m3u` — M3U/M3U8 playlist format

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
