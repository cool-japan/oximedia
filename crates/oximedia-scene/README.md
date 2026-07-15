# oximedia-scene

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Scene understanding and AI-powered video analysis for OxiMedia. Provides comprehensive scene understanding using patent-free algorithms for classification, object detection, activity recognition, composition analysis, and semantic segmentation.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 | Tests: extensively tested — 2026-07-14

## Features

- **Scene Classification** - Indoor/outdoor, day/night, landscape, portrait, and content type classification
- **Object Detection** - HOG-based patent-free object detection
- **Activity Recognition** - Recognize activities (walking, running, sports) via motion histograms
- **Shot Composition** - Analyze framing (rule of thirds, symmetry, leading lines)
- **Semantic Segmentation** - Graph-based segmentation of semantic regions (sky, ground, people)
- **Saliency Detection** - Spectral saliency for visually important regions
- **Aesthetic Scoring** - Rate aesthetic quality of frames
- **Event Detection** - Sports and live content event detection
- **Face Detection** - Haar cascade face detection
- **Logo Detection** - Brand logo and graphics detection
- **Camera Motion** - Detect and classify camera motion
- **Storyboard Generation** - Automatic storyboard from video
- **Scene Boundary Detection** - Detect scene cuts and transitions
- **Visual Rhythm Analysis** - Temporal pacing and rhythm detection
- **Crowd Density Estimation** - Estimate crowd density in scenes
- **Depth of Field Analysis** - Detect focus plane and bokeh
- **Lighting Analysis** - Analyze lighting conditions and quality
- **Color Temperature** - Detect and classify color temperature
- **Scene Mood** - Emotional and mood classification
- **Continuity Checking** - Detect continuity errors between shots
- **Pacing Analysis** - Editorial pacing and tempo analysis
- **Location Classification** - Geographic and context location detection

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-scene = "0.2.0"
```

```rust
use oximedia_scene::classify::scene::SceneClassifier;
use oximedia_scene::detect::face::FaceDetector;
use oximedia_scene::composition::rules::CompositionAnalyzer;

let classifier = SceneClassifier::new();
let face_detector = FaceDetector::new();
let composition = CompositionAnalyzer::new();
```

## API Overview

- `SceneError` / `SceneResult` — Error and result types
- `common::Point` / `common::Rect` / `common::Confidence` — Geometric primitives
- Modules: `action_beat`, `activity`, `aesthetic`, `camera_motion`, `classification`, `classify`, `color_temperature`, `composition`, `continuity_check`, `crowd_density`, `depth_of_field`, `detect`, `error`, `event`, `features`, `lighting_analysis`, `location`, `mood`, `pacing`, `saliency`, `scene_boundary`, `scene_graph`, `scene_metadata`, `scene_score`, `scene_stats`, `segment`, `segmentation`, `shot_type`, `storyboard`, `summarization`, `transition`, `visual_rhythm`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
