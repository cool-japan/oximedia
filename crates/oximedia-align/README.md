# oximedia-align

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Video alignment and registration tools for multi-camera synchronization in OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Temporal Alignment
- **Audio Cross-Correlation** — Precise synchronization using audio tracks
- **Timecode Synchronization** — LTC/VITC-based alignment
- **Visual Markers** — Clapper detection and flash-based sync
- **Sub-frame Accuracy** — Timing precision down to microseconds

### Spatial Registration
- **Homography Estimation** — Planar perspective transformation with RANSAC
- **Perspective Correction** — Remove keystone distortion
- **Feature Matching** — Robust point correspondence between views
- **Affine Transforms** — Translation, rotation, scaling, and shear

### Feature Detection
- **FAST Corners** — High-speed corner detection
- **BRIEF Descriptors** — Binary robust independent elementary features
- **ORB Features** — Oriented FAST and Rotated BRIEF
- **Patent-Free** — All algorithms are free from patent restrictions

### Lens Distortion Correction
- **Brown-Conrady Model** — Radial and tangential distortion
- **Fisheye Model** — Wide-angle lens correction
- **Camera Calibration** — Intrinsic parameter estimation
- **Real-time Undistortion** — Precomputed lookup tables

### Color Matching
- **Color Transfer** — Match color distributions across cameras
- **Histogram Matching** — Equalize color histograms
- **White Balance** — Illuminant estimation (gray world, white patch)
- **ColorChecker Calibration** — Industry-standard color calibration

### Sync Markers
- **Clapper Detection** — Automatic slate detection
- **Flash Detection** — Bright flash synchronization
- **LED Markers** — Coded light patterns
- **Audio Spike Detection** — Sharp transient detection

### Rolling Shutter Correction
- **Motion Estimation** — Per-scanline motion vectors
- **Wobble Correction** — Remove rolling shutter wobble
- **Skew Removal** — Correct geometric distortion
- **Global Shutter Simulation** — Temporal interpolation

### Geometric Transformations
- **Image Warping** — Homography and affine transforms
- **Interpolation** — Nearest, bilinear, and bicubic
- **Mesh Warping** — Non-rigid deformations
- **Border Handling** — Constant, replicate, reflect, and wrap modes

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-align = "0.2.0"
```

### Audio-Based Synchronization

```rust
use oximedia_align::temporal::{AudioSync, SyncConfig};

let config = SyncConfig {
    sample_rate: 48000,
    window_size: 480000,  // 10 seconds
    max_offset: 240000,   // ±5 seconds
};

let sync = AudioSync::new(config);
let offset = sync.find_offset(&audio1, &audio2)?;
println!("Offset: {} samples ({:.2} ms)",
    offset.samples,
    offset.to_milliseconds(48000)
);
```

### Homography Estimation

```rust
use oximedia_align::spatial::{HomographyEstimator, RansacConfig};
use oximedia_align::features::{OrbDetector, FeatureMatcher};

let orb = OrbDetector::new(500);
let (kp1, desc1) = orb.detect_and_compute(&img1, width, height)?;
let (kp2, desc2) = orb.detect_and_compute(&img2, width, height)?;

let matcher = FeatureMatcher::default();
let matches = matcher.match_features(&kp1, &desc1, &kp2, &desc2);

let estimator = HomographyEstimator::new(RansacConfig::default());
let (homography, inliers) = estimator.estimate(&matches)?;
```

### Lens Distortion Correction

```rust
use oximedia_align::distortion::{
    CameraIntrinsics, BrownConradyDistortion,
    CameraModel, DistortionModel, ImageUndistorter
};

let intrinsics = CameraIntrinsics::new(1000.0, 1000.0, 640.0, 480.0);
let distortion = BrownConradyDistortion::new(0.1, 0.01, 0.001, 0.0, 0.0);
let camera = CameraModel::new(intrinsics, DistortionModel::BrownConrady(distortion));
let undistorter = ImageUndistorter::new(camera, 1280, 960);
let corrected = undistorter.undistort(&distorted_image, 3)?;
```

### Rolling Shutter Correction

```rust
use oximedia_align::rolling_shutter::{
    RollingShutterParams, RollingShutterCorrector, ReadoutDirection
};

let params = RollingShutterParams::new(
    0.033,  // 33ms readout time
    30.0,   // 30 fps
    ReadoutDirection::TopToBottom
);
let corrector = RollingShutterCorrector::new(params);
let corrected = corrector.estimate_and_correct(&frame1, &frame2, width, height)?;
```

## API Overview

**Modules (36 source files, 731 public items):**
- `temporal` — Audio cross-correlation and timecode synchronization
- `spatial` — Homography estimation and RANSAC-based registration
- `features` — ORB/FAST feature detection and matching
- `distortion` — Lens distortion models and correction
- `color` — Color transfer and white balance
- `markers` — Sync marker detection (clapper, flash, LED)
- `rolling_shutter` — Rolling shutter correction
- `warp` — Geometric transformation and image warping

## Safety

This crate uses `#![forbid(unsafe_code)]` and contains no unsafe operations.

## Patent-Free

All algorithms are selected to avoid patent-encumbered methods. ORB features are used instead of SIFT/SURF.

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
