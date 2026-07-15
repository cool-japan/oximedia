# oximedia-stabilize

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional video stabilization for OxiMedia. Provides comprehensive video stabilization algorithms including motion estimation, trajectory smoothing, rolling shutter correction, 3D stabilization, and horizon leveling.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Motion Estimation** - Track camera motion across frames (translation, affine, perspective, full 3D)
- **Motion Smoothing** - Gaussian, Kalman, and adaptive smoothing algorithms
- **Transform Calculation** - Compute optimal stabilization transforms
- **Frame Warping** - Apply stabilization with various interpolation methods
- **Rolling Shutter Correction** - Fix rolling shutter CMOS artifacts
- **3D Stabilization** - Full 3D camera motion estimation and correction
- **Horizon Leveling** - Automatic horizon detection and correction
- **Zoom Optimization** - Minimize black borders while maximizing output resolution
- **Motion Blur** - Optional synthetic motion blur for smooth results
- **Multi-pass Analysis** - Analyze entire video before stabilizing for optimal results
- **Quality Presets** - Fast, Balanced, Maximum quality presets
- **Active Camera Motion** - Distinguish intentional from unwanted motion
- **Gyroscope Integration** - Use gyro data for improved stabilization
- **Warp Field** - Dense warp field-based stabilization
- **Parallax Compensation** - Correct for parallax in complex scenes
- **Perspective Warp** - Perspective-correcting warp for extreme camera angles
- **Keyframe Filtering** - Filter stabilization keyframes for smoothness
- **Trajectory Analysis** - Analyze and correct camera trajectory
- **Adaptive Crop** - Dynamically crop to hide stabilization borders
- **Multipass Analysis** - Multi-pass analysis for best results

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-stabilize = "0.2.0"
```

```rust
use oximedia_stabilize::{Stabilizer, StabilizeConfig, StabilizationMode, QualityPreset};

let config = StabilizeConfig {
    mode: StabilizationMode::Affine,
    quality: QualityPreset::Balanced,
    ..Default::default()
};

let mut stabilizer = Stabilizer::new(config)?;
// stabilizer.analyze_video(frames)?;
// let stabilized = stabilizer.stabilize_frame(frame)?;
```

## API Overview

- `Stabilizer` — Main stabilizer with analysis and per-frame processing
- `StabilizeConfig` — Configuration with mode, quality, and algorithm settings
- `StabilizationMode` — TranslationOnly, Affine, Perspective, ThreeD
- `QualityPreset` — Fast, Balanced, Maximum
- Modules: `adaptive_crop`, `crop_region`, `gyro`, `horizon`, `keyframe_filter`, `motion`, `motion_model`, `multipass`, `parallax_compensate`, `perspective_warp`, `smooth`, `trajectory`, `warp_field`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
