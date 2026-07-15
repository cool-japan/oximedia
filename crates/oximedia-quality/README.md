# oximedia-quality

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Video quality assessment and objective metrics for OxiMedia. Provides comprehensive video quality assessment including full-reference metrics (PSNR, SSIM, VMAF) and no-reference metrics (NIQE, BRISQUE, blur, noise).

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

### Full-Reference Metrics
- **PSNR** - Peak Signal-to-Noise Ratio
- **SSIM** - Structural Similarity Index
- **MS-SSIM** - Multi-Scale SSIM
- **VMAF** - Video Multi-Method Assessment Fusion
- **VIF** - Visual Information Fidelity
- **FSIM** - Feature Similarity Index

### No-Reference Metrics
- **NIQE** - Natural Image Quality Evaluator
- **BRISQUE** - Blind/Referenceless Image Spatial Quality Evaluator
- **Blockiness** - DCT-based blockiness detection
- **Blur** - Laplacian variance and edge width measurement
- **Noise** - Spatial/temporal noise estimation

### Additional Capabilities
- **Batch Assessment** - Evaluate entire video sequences efficiently
- **Temporal Pooling** - Mean, harmonic mean, minimum, percentile pooling
- **Quality Presets** - Predefined quality gates for delivery validation
- **Quality Reports** - Detailed per-frame and aggregate reporting
- **Aggregate Score** - Combined quality scoring across multiple metrics
- **Artifact Score** - Compression artifact scoring
- **Bitrate Quality** - Bitrate-to-quality relationship analysis
- **Codec Quality** - Codec-specific quality assessment
- **Color Fidelity** - Color accuracy measurement
- **Compression Artifacts** - Artifact detection and measurement
- **Histogram Quality** - Histogram-based quality metrics
- **Perceptual Model** - Perceptual quality modeling
- **Quality Gate** - Pass/fail quality thresholds
- **Scene Quality** - Per-scene quality assessment
- **Sharpness Score** - Sharpness and focus measurement
- **Spatial Quality** - Spatial resolution quality
- **Temporal Quality** - Temporal consistency metrics
- **VMAF Score** - VMAF scoring model

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-quality = "0.2.0"
```

```rust
use oximedia_quality::{QualityAssessor, MetricType, Frame};
use oximedia_core::PixelFormat;

let assessor = QualityAssessor::new();

let reference = Frame::new(1920, 1080, PixelFormat::Yuv420p)?;
let distorted = Frame::new(1920, 1080, PixelFormat::Yuv420p)?;

// Full-reference assessment
let ssim_score = assessor.assess(&reference, &distorted, MetricType::Ssim)?;
println!("SSIM: {}", ssim_score.score);

// No-reference assessment
let blur_score = assessor.assess_no_reference(&distorted, MetricType::Blur)?;
```

## API Overview

**Core types:**
- `QualityAssessor` — Unified access to all quality metrics
- `Frame` — Video frame with planar pixel data (luma/chroma access)
- `MetricType` — Available metrics: Psnr, Ssim, MsSsim, Vmaf, Vif, Fsim, Niqe, Brisque, Blockiness, Blur, Noise
- `QualityScore` — Score result with per-component values and optional frame number
- `PoolingMethod` — Temporal aggregation: Mean, HarmonicMean, Min, Percentile
- `BatchAssessment` — Batch video sequence assessment
- `ReferenceManager` — Reference frame management

**Full-reference calculators:**
- `PsnrCalculator` — PSNR calculation
- `SsimCalculator` — SSIM calculation
- `MsSsimCalculator` — MS-SSIM calculation
- `VmafCalculator` — VMAF calculation
- `VifCalculator` — VIF calculation
- `FsimCalculator` — FSIM calculation

**No-reference assessors:**
- `NiqeAssessor` — NIQE assessment
- `BrisqueAssessor` — BRISQUE assessment
- `BlockinessDetector` — Blockiness detection
- `BlurDetector` — Blur detection
- `NoiseEstimator` — Noise estimation

**Public modules:**
- `aggregate_score` — Combined score aggregation
- `artifact_score` — Artifact scoring
- `bitrate_quality` — Bitrate/quality relationship
- `codec_quality` — Codec-specific quality
- `color_fidelity` — Color accuracy
- `compression_artifacts` — Artifact analysis
- `histogram_quality` — Histogram quality metrics
- `metrics` — Core metric types
- `perceptual`, `perceptual_model` — Perceptual quality models
- `quality_gate` — Quality gating (pass/fail)
- `quality_preset` — Quality presets
- `quality_report` — Report generation
- `reference_free` — No-reference assessment utilities
- `scene_quality` — Per-scene quality
- `sharpness_score` — Sharpness measurement
- `spatial_quality` — Spatial quality
- `temporal_quality` — Temporal consistency
- `vmaf_score` — VMAF scoring

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
