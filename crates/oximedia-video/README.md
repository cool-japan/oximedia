# oximedia-video

Professional video processing operations for OxiMedia — motion compensation, frame interpolation, and deinterlacing

[![Crates.io](https://img.shields.io/crates/v/oximedia-video.svg)](https://crates.io/crates/oximedia-video)
[![Documentation](https://docs.rs/oximedia-video/badge.svg)](https://docs.rs/oximedia-video)
[![License](https://img.shields.io/crates/l/oximedia-video.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

## Features

- Deinterlacing with Weave, Bob, Blend, and YADIF-inspired adaptive methods
- Block-based motion estimation with FullSearch, ThreeStep, DiamondSearch, and HexagonSearch algorithms
- Frame interpolation via blending, motion-based warping, duplicate, and drop strategies
- Scene change detection using histogram, edge, threshold, and adaptive methods with gradual transition support
- 3:2 pulldown cadence detection and inverse telecine with combing score analysis
- Perceptual video fingerprinting with DCT 8x8, Average, Difference, and Wavelet hash methods
- Temporal noise reduction using Donoho-Johnstone sigma estimation with adaptive and motion-compensated modes
- `#![forbid(unsafe_code)]` -- fully safe Rust

## Quick Start

```toml
[dependencies]
oximedia-video = "0.2.0"
```

```rust
use oximedia_video::{
    SceneChangeDetector, SceneDetectionMethod,
    Deinterlacer, DeinterlaceMethod, FieldOrder,
    MotionEstimator, MeAlgorithm,
};

// Detect scene changes using adaptive histogram comparison
let mut detector = SceneChangeDetector::new(SceneDetectionMethod::Adaptive, 0.35);
let change = detector.detect_change(&prev_frame, &curr_frame);
if let Some(sc) = change {
    println!("Scene change at frame {}: {:?}", sc.frame_index, sc.change_type);
}

// Deinterlace a field-based frame using YADIF-inspired adaptive filtering
let deinterlacer = Deinterlacer::new(DeinterlaceMethod::Yadif, FieldOrder::TopFirst);
let progressive = deinterlacer.process(&interlaced_frame);

// Estimate motion vectors using diamond search
let estimator = MotionEstimator::new(MeAlgorithm::DiamondSearch, 16, 16);
let vectors = estimator.estimate(&reference_frame, &current_frame);
```

## Modules

### `deinterlace`

Converts interlaced video to progressive using four methods. Key types:

- `DeinterlaceMethod` -- `Weave | Bob | Blend | Yadif` (YADIF uses spatial-temporal adaptive interpolation)
- `FieldOrder` -- `TopFirst | BottomFirst`
- `Deinterlacer` -- stateful processor that applies the selected method
- `split_fields()` -- separates a frame into top and bottom fields

### `motion_compensation`

Block-matching motion estimation and frame compensation. Key types:

- `MotionVector` -- displacement vector with SAD cost for a macroblock
- `MeAlgorithm` -- `FullSearch | ThreeStep | DiamondSearch | HexagonSearch`
- `MotionEstimator` -- configurable block size and search range
- `compute_sad()` -- Sum of Absolute Differences between blocks
- `compensate_frame()` -- reconstructs a frame from reference + motion vectors
- `residual_frame()` / `reconstruct_from_residual()` -- residual coding support

### `frame_interpolation`

Generates intermediate frames for frame rate conversion. Key types:

- `FrameInterpolationMethod` -- `Blend | MotionBased | Duplicate | Drop`
- `FrameInterpolator` -- produces intermediate frames given two references and a temporal position
- `InterpResult` -- interpolated frame with metadata

### `scene_detection`

Detects hard cuts and gradual transitions between scenes. Key types:

- `SceneDetectionMethod` -- `ThresholdBased | HistogramBased | EdgeBased | Adaptive`
- `FrameFeatures` -- extracted luminance histogram and edge density
- `SceneChangeType` -- `HardCut | GradualTransition | Unknown`
- `SceneChange` / `SceneBoundary` -- detected change events with confidence scores
- `SceneIndex` -- full scene segmentation of a video
- `SceneChangeDetector` -- stateful detector with configurable threshold
- `extract_features()` / `detect_change()` / `detect_gradual()` -- analysis functions

### `pulldown_detect`

Identifies 3:2 pulldown cadence patterns and performs inverse telecine. Key types:

- `Cadence` -- `Progressive | Interlaced | Pulldown23 | Pulldown32 | Pulldown2332`
- `FieldPair` / `FieldMetrics` -- per-field combing and motion measurements
- `ProgressiveFrame` -- output of inverse telecine
- `CadenceDetector` -- sliding-window cadence classifier
- `combing_score()` -- measures interlace artifacts in a frame
- `detect_cadence()` -- identifies the active pulldown pattern
- `remove_pulldown()` -- reconstructs progressive frames from telecined input
- `split_into_field_pair()` -- separates a frame into field pairs for analysis

### `video_fingerprint`

Perceptual hashing for duplicate detection and content identification. Key types:

- `FingerprintMethod` -- `DCT8x8 | Average | Difference | Wavelet`
- `FrameFingerprint` -- per-frame hash with timestamp
- `VideoFingerprint` -- sequence of frame fingerprints for a clip
- `FingerprintMatch` -- match result with similarity score and frame mapping
- `FingerprintMatcher` -- compares fingerprints with configurable distance threshold
- `compute_hash()` -- generates a perceptual hash for a single frame
- `hamming_distance()` / `similarity()` -- hash comparison functions

### `temporal_denoise`

Temporal noise reduction using multi-frame averaging with motion awareness. Key types:

- `TemporalDenoiseMode` -- `Adaptive | Fixed | MotionCompensated`
- `NoiseMetrics` -- estimated noise sigma and SNR
- `TemporalDenoiser` -- stateful denoiser with frame history ring buffer
- `blend_frames()` -- weighted temporal averaging
- `adaptive_blend_factor()` -- adjusts blending strength based on local motion
- `motion_score_between()` -- inter-frame motion magnitude
- `denoise_frame()` -- single-call denoising entry point
- `estimate_noise_sigma()` -- Donoho-Johnstone robust noise estimation

## Architecture

The crate provides seven independent video processing modules that can be composed into
complete post-production pipelines. The `SceneChangeDetector` segments video at shot
boundaries, feeding into per-shot processing chains. `Deinterlacer` and `CadenceDetector`
handle legacy interlaced content, while `MotionEstimator` provides motion vectors used by
both `FrameInterpolator` (frame rate conversion) and `TemporalDenoiser` (motion-compensated
noise reduction). `FingerprintMatcher` enables content identification across archives.

All modules share a common `VideoError` type. No unsafe code is used anywhere in the crate.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)

Version: 0.2.0 — 2026-07-15 — extensively tested
