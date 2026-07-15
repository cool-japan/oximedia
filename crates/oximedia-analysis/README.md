# oximedia-analysis

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Comprehensive media analysis and quality assessment tools for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-analysis` provides professional-grade tools for analyzing video and audio content, detecting quality issues, classifying content types, and generating detailed reports. It is designed for broadcast quality control, content verification, and automated media inspection workflows.

## Features

### Video Analysis

- **Scene Detection** — Automatically detect scene changes using histogram difference, edge change ratio (ECR), and motion-compensated comparison
- **Black Frame Detection** — Identify black or near-black frames with configurable luminance threshold and minimum duration filtering
- **Quality Assessment** — No-reference (blind) quality metrics: blockiness via DCT analysis, blur via Laplacian variance, noise estimation
- **Content Classification** — Action, static, talking head, sports, and animation detection
- **Thumbnail Generation** — Quality-based, temporally diverse frame selection
- **Motion Analysis** — Global motion estimation (pans, tilts, zooms), local motion energy, camera stability measurement
- **Color Analysis** — Dominant color extraction (K-means clustering), color grading style detection, saturation analysis
- **Temporal Analysis** — Flicker detection, judder identification, temporal noise estimation, telecine pattern detection

### Audio Analysis

- **Silence Detection** — Identify silent or near-silent segments
- **Loudness Analysis** — ITU-R BS.1770-4 compliant (via oximedia-metering)
- **Clipping Detection** — Digital clipping and distortion detection
- **Phase Correlation** — Stereo phase issues and mono compatibility
- **Spectral Analysis** — FFT-based frequency content: spectral centroid, flatness, rolloff, band energy ratios
- **Dynamic Range** — Peak-to-RMS ratio measurement

### Report Generation

- **JSON Reports** — Machine-readable structured data
- **HTML Reports** — Human-readable visual reports with quality charts, scene timelines, and statistics

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-analysis = "0.2.0"
```

### Quick Start

```rust
use oximedia_analysis::{Analyzer, AnalysisConfig};
use oximedia_core::types::Rational;

let config = AnalysisConfig::default()
    .with_scene_detection(true)
    .with_quality_assessment(true)
    .with_black_frame_detection(true);

let mut analyzer = Analyzer::new(config);

// Process video frames (YUV420p format)
for frame_num in 0..frame_count {
    analyzer.process_video_frame(
        &y_plane,
        &u_plane,
        &v_plane,
        width,
        height,
        Rational::new(30, 1),
    )?;
}

// Process audio samples
analyzer.process_audio_samples(&audio_samples, 48000)?;

// Get results and generate reports
let results = analyzer.finalize();
let json_report = results.to_json()?;
let html_report = results.to_html()?;
```

### Configuration

```rust
let config = AnalysisConfig::new()
    .with_scene_detection(true)
    .with_quality_assessment(true)
    .with_black_frame_detection(true)
    .with_content_classification(true)
    .with_thumbnail_generation(10)
    .with_motion_analysis(true)
    .with_color_analysis(true)
    .with_audio_analysis(true)
    .with_temporal_analysis(true);
```

### Quality Assessment

```rust
use oximedia_analysis::quality::QualityAssessor;

let mut assessor = QualityAssessor::new();
for frame_num in 0..frame_count {
    assessor.process_frame(&y_plane, width, height, frame_num)?;
}
let stats = assessor.finalize();
println!("Average quality: {:.2}", stats.average_score);
println!("Blockiness: {:.3}", stats.avg_blockiness);
```

## Architecture

### Modular Design (43 source files, 571 public items)

Each analysis module is independent and usable standalone:

- `scene::SceneDetector` — Scene change detection
- `quality::QualityAssessor` — Video quality metrics
- `black::BlackFrameDetector` — Black frame detection
- `content::ContentClassifier` — Content classification
- `thumbnail::ThumbnailSelector` — Thumbnail selection
- `motion::MotionAnalyzer` — Motion analysis
- `color::ColorAnalyzer` — Color analysis
- `audio::AudioAnalyzer` — Audio analysis
- `temporal::TemporalAnalyzer` — Temporal artifacts

### Single-Pass Processing

All analyzers operate in a single pass for efficiency. Video frames are processed once, with all enabled analyses running concurrently via rayon parallel iteration.

## Performance

| Resolution  | Operation        | FPS  |
|-------------|------------------|------|
| 1920x1080   | Scene detection  | ~200 |
| 1920x1080   | Quality assess.  | ~150 |
| 1920x1080   | Full pipeline    | ~80  |
| 3840x2160   | Scene detection  | ~50  |

## Use Cases

- Broadcast quality control and pre-transmission validation
- Post-production dailies review
- Video platform upload validation and content tagging
- Media asset management cataloging

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
