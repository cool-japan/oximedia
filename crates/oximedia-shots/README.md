# oximedia-shots

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Advanced shot detection and classification engine for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.1 | Tests: extensively tested — 2026-08-12

## Features

- **Shot Detection**: Automatic detection of hard cuts, dissolves, fades, and wipes
- **Shot Classification**: Classify shots into types (ECU, CU, MCU, MS, MLS, LS, ELS)
- **Camera Analysis**: Detect camera angles and movements (pan, tilt, zoom, dolly, handheld)
- **Composition Analysis**: Evaluate rule of thirds, symmetry, balance, leading lines, and depth
- **Scene Detection**: Automatically group shots into scenes
- **Coverage Analysis**: Identify master shots, singles, two-shots, and other coverage types
- **Continuity Checking**: Detect jump cuts and other continuity errors
- **Pattern Analysis**: Identify shot-reverse-shot, montage sequences, and editing rhythms
- **Quality Metrics**: Assess shot quality and detect potential issues
- **Export**: Generate shot lists (CSV, JSON) and EDL files with metadata
- **Visualization**: Create ASCII and SVG timeline visualizations
- **Shot Grouping**: Semantic grouping of related shots
- **Shot Matching**: Match similar shots across a production
- **Shot Rhythm Analysis**: Editorial pacing and tempo detection
- **Shot Report Generation**: Comprehensive shot analysis reports
- **Framing Guide**: Rule-based framing quality assessment
- **Storyboard Export**: Generate storyboard images from shots
- **Transition Analysis**: Analyze transition types and timing
- **Shot Rating**: Automated quality rating system
- **Coverage Map**: Visual coverage analysis across a scene

## Shot Types

| Type | Abbreviation | Description |
|------|--------------|-------------|
| Extreme Close-up | ECU | Face details, eyes, lips |
| Close-up | CU | Head and shoulders |
| Medium Close-up | MCU | Waist up |
| Medium Shot | MS | Knees up |
| Medium Long Shot | MLS | Full body with space |
| Long Shot | LS | Full body in environment |
| Extreme Long Shot | ELS | Establishing shot |

## Camera Movements

- **Pan**: Left/right horizontal camera movement
- **Tilt**: Up/down vertical camera movement
- **Zoom**: Lens zoom in/out (optical)
- **Dolly**: Camera moving toward/away from subject
- **Track**: Lateral camera movement
- **Handheld**: Unstabilized camera shake

## Usage

```rust
use oximedia_shots::{ShotDetector, ShotDetectorConfig};

// Create detector with custom configuration
let config = ShotDetectorConfig {
    enable_cut_detection: true,
    enable_classification: true,
    enable_movement_detection: true,
    enable_composition_analysis: true,
    ..Default::default()
};

let detector = ShotDetector::new(config);

// Process video frames
// let shots = detector.detect_shots(&frames)?;

// Analyze results
// let statistics = detector.analyze_shots(&shots);
// let scenes = detector.detect_scenes(&shots);
// let issues = detector.check_continuity(&shots);
```

## API Overview

- `ShotDetector` — Main shot detection engine with configurable detection capabilities
- `ShotDetectorConfig` — Enable/disable detection modes and configure thresholds
- `Shot` — Shot data including type, angle, composition, movements, confidence
- `ShotType` — ECU, CU, MCU, MS, MLS, LS, ELS
- `CameraAngle` — High, EyeLevel, Low, BirdsEye, Dutch
- `CameraMovement` / `MovementType` — Pan, Tilt, Zoom, Dolly, Track, Handheld
- `CompositionAnalysis` — Rule of thirds, symmetry, balance, leading lines scores
- `CoverageType` — Master, Single, TwoShot, OverShoulder
- `Scene` — Group of related shots
- `ShotStatistics` — Aggregated statistics across all shots
- `TransitionType` — Cut, Dissolve, Fade, Wipe
- `ShotError` / `ShotResult` — Error and result types
- Modules: `analysis`, `boundary`, `camera`, `camera_movement`, `classification`, `classify`, `composition`, `continuity`, `coverage`, `coverage_map`, `detect`, `detector`, `duration`, `error`, `export`, `framing`, `framing_guide`, `log`, `metrics`, `pacing`, `pattern`, `rating`, `scene`, `scene_graph`, `shot_grouping`, `shot_matching`, `shot_report`, `shot_rhythm`, `shot_stats`, `shot_transition`, `shot_type`, `storyboard`, `transition_analysis`, `types`, `visualize`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
