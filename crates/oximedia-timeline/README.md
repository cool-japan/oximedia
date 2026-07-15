# oximedia-timeline

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Multi-track timeline editor for OxiMedia. Provides a professional-grade timeline editor with support for multi-track video/audio editing, frame-accurate operations, keyframe animation, transitions, professional editing tools, and EDL/XML/AAF import/export.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Multi-track Editing** - Video and audio tracks with clips and gaps
- **Frame-accurate Operations** - Precise editing at individual frame level
- **Professional Editing** - Slip, slide, roll, and ripple edit operations
- **Transitions** - Dissolve, wipe, push, and custom transitions with full engine
- **Effects** - Effect stack with keyframe animation
- **Keyframe Animation** - Smooth keyframe interpolation for all parameters
- **Multi-camera** - Multi-cam angle editing within timeline
- **Nested Sequences** - Compound clips and nested timelines
- **Markers** - Frame-accurate markers with metadata
- **EDL Import/Export** - CMX 3600 EDL format support
- **XML Import/Export** - FCP XML and Premiere XML
- **AAF Import/Export** - Advanced Authoring Format
- **Real-time Playback** - Playback engine with caching
- **Render Queue** - Background rendering with priority
- **Color Correction Track** - Dedicated color grading track type
- **Track Groups** - Group and manage related tracks
- **Track Routing** - Audio routing per track
- **Track Colors** - Visual track identification
- **Snap Grid** - Magnetic snap grid for precise editing
- **Razor Tool** - Frame-accurate clip splitting
- **Gap Filler** - Automatic gap detection and filling
- **Timeline Diff** - Compare two timeline versions
- **Timeline Events** - Event-based timeline notification system
- **Timeline Lock** - Prevent accidental edits to locked tracks
- **Version Snapshots** - Save and restore timeline versions
- **Clip Sequences** - Order and manage clip sequences
- **Sequence Range** - Define and operate on sequence ranges
- **Compound Regions** - Complex multi-clip compound regions
- **Export Settings** - Configurable export presets
- **Conform** - Conform timeline to updated media
- **Audio Mixer** - Integrated multi-track audio mixing

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-timeline = "0.2.0"
```

```rust
use oximedia_timeline::{Timeline, Clip, MediaSource, Transition, TransitionType};
use oximedia_timeline::types::{Duration, Position};
use oximedia_core::Rational;

// Create a new timeline
let mut timeline = Timeline::new(
    "My Project",
    Rational::new(24000, 1001), // 23.976 fps
    48000,                       // 48kHz audio
)?;

let video_track = timeline.add_video_track("Video 1")?;
let clip = Clip::new(
    "My Clip".to_string(),
    MediaSource::black(),
    Position::new(0),
    Position::new(100),
    Position::new(0),
)?;
let clip_id = clip.id;
timeline.insert_clip(video_track, clip, Position::new(0))?;

// Add a dissolve transition
let transition = Transition::dissolve(Duration::new(24));
timeline.add_transition(clip_id, transition)?;
```

## API Overview

- `Timeline` — Multi-track timeline container
- `Track` / `TrackId` / `TrackType` — Track management
- `Clip` / `ClipId` / `MediaSource` — Clip data model
- `Transition` / `TransitionType` / `TransitionAlignment` — Transitions
- `TransitionEngine` / `TransitionInput` — Transition rendering
- `Effect` / `EffectId` / `EffectStack` — Effects system
- `Keyframe` / `KeyframeInterpolation` / `KeyframeValue` — Keyframe animation
- `EditOperation` / `EditMode` — Editing operation types
- `TimelineExporter` / `EdlExportOptions` / `EdlEvent` — EDL/XML export
- `TimelineRenderer` / `RenderedFrame` / `PixelBuffer` — Frame rendering
- `TrackMixer` / `TrackMixParams` / `AudioFrame` / `MixResult` — Audio mixing
- `Marker` / `MarkerType` — Timeline markers
- `TimecodeValue` / `TimecodeFormat` — Timecode representation
- `Duration` / `Position` / `Speed` — Timeline measurement types
- `TimelineError` / `TimelineResult` — Error and result types
- Modules: `audio`, `cache`, `clip`, `clip_metadata`, `clip_sequence`, `color_correction_track`, `compound_clip`, `compound_region`, `conform`, `edit`, `effects`, `error`, `export`, `export_settings`, `gap_filler`, `import`, `keyframe`, `keyframe_animation`, `marker`, `markers`, `metadata`, `mixer`, `multicam`, `nested`, `nested_compound`, `nested_timeline`, `playback`, `razor_tool`, `render`, `render_queue`, `renderer`, `sequence`, `sequence_range`, `snap_grid`, `timecode`, `timeline`, `timeline_diff`, `timeline_event`, `timeline_exporter`, `timeline_lock`, `track`, `track_color`, `track_group`, `track_routing`, `transition`, `transition_engine`, `types`, `version_snapshot`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
