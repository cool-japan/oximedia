# oximedia-multicam

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Multi-camera synchronization, switching, and editing for OxiMedia. Provides comprehensive multi-camera production capabilities including temporal synchronization, AI-based automatic switching, multi-view composition, color matching, and spatial alignment.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Temporal Synchronization** - Frame-accurate sync across cameras using audio cross-correlation, timecode (LTC/VITC/SMPTE), visual markers, and genlock simulation
- **Drift Correction** - Detect and correct sync drift over time
- **Multi-angle Editing** - Timeline with angle switching and smooth transitions
- **Automatic Switching** - AI-based camera selection with speaker detection and action following rules engine
- **Manual Control** - Manual switching interface with simultaneous angle preview
- **Composition** - Picture-in-Picture, split-screen, and grid layouts (2x2, 3x3, 4x4)
- **Color Matching** - Match color appearance and white balance across camera angles
- **Spatial Alignment** - Align overlapping views and stitch panoramas
- **ISO Recording** - Isolated camera recording and sync
- **Tally System** - Tally light integration for live production
- **Metadata Tracking** - Per-angle metadata, markers, and cue points
- **Angle Scoring** - Automated angle priority scoring
- **Bank System** - Camera bank management for quick switching
- **Genlock Master** - Genlock master synchronization
- **Replay Buffer** - Replay buffer for instant replay
- **Timecode Sync** - Multi-camera timecode synchronization
- **Switch List** - Switch list for automated switching sequences
- **Cut Analysis** - Automated cut point analysis and suggestion

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-multicam = "0.2.0"
```

```rust
use oximedia_multicam::edit::MultiCamTimeline;
use oximedia_multicam::sync::SyncMethod;
use oximedia_multicam::auto::{AutoSwitcher, SwitchingRule};
use oximedia_multicam::composite::Layout;

// Create a multi-camera timeline with 3 angles
let mut timeline = MultiCamTimeline::new(3);

// Create auto-switcher with rules
let mut switcher = AutoSwitcher::new();
switcher.add_rule(SwitchingRule::SpeakerDetection { sensitivity: 0.8 });
switcher.add_rule(SwitchingRule::ActionFollowing { smoothness: 0.7 });
```

## API Overview

**Core types:**
- `MultiCamTimeline` — Multi-angle timeline with angle switching
- `AutoSwitcher` — AI-based automatic camera angle selection
- `SwitchingRule` — Rules for automatic switching (speaker detection, action following)
- `MultiCamConfig` — Session configuration with sync options
- `CameraInfo` / `CameraPosition` — Camera metadata and 3D positioning
- `SyncStatus` — Synchronization state tracking

**Modules:**
- `angle`, `angle_group`, `angle_priority`, `angle_score`, `angle_sync`, `angle_sync_ext` — Angle management
- `auto` — Automatic switching engine
- `bank_ctrl`, `bank_system` — Camera bank management
- `cam_label`, `cam_metadata` — Camera labeling and metadata
- `clip_split` — Multi-cam clip splitting
- `color` — Color matching across angles
- `composite` — Composition layouts (PiP, split-screen, grid)
- `coverage_map` — Camera coverage mapping
- `cut_analysis`, `cut_point` — Cut detection and analysis
- `edit`, `edit_decision` — Multi-cam editing
- `error` — Error types
- `genlock_master` — Genlock master synchronization
- `iso_file_sync`, `iso_record`, `iso_recording`, `iso_sync` — ISO recording management
- `manual` — Manual switching control
- `metadata` — Session metadata
- `multicam_export` — Multi-cam project export
- `replay_buffer` — Instant replay buffer
- `spatial` — Spatial alignment and panorama stitching
- `switch_list`, `switcher` — Switch list and switcher control
- `sync`, `sync_report` — Synchronization engine and reporting
- `tally_system` — Tally light integration
- `timecode_sync` — Timecode-based synchronization

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
