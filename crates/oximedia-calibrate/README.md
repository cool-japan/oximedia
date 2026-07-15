# oximedia-calibrate

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional color calibration and matching tools for OxiMedia, enabling camera profiling, display calibration, ICC profile generation, and multi-device color matching.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Camera Calibration** — ColorChecker-based camera profiling with subpixel patch extraction
- **Display Calibration** — Gamma curve measurement, uniformity testing, monitor profiling
- **Color Matching** — Multi-camera color matching, scene-to-scene matching, reference target matching
- **ICC Profile Generation** — ICC v2/v4 profile creation and application
- **LUT Generation** — Measurement-based 1D and 3D calibration LUT creation
- **White Balance** — Automatic white balance, standard presets, gray world and white patch algorithms
- **Color Temperature** — Automatic estimation, Kelvin-to-RGB conversion, illuminant D-series support
- **Gamut Mapping** — Device gamut to working space mapping, perceptual gamut compression
- **Chromatic Adaptation** — Bradford and Von Kries chromatic adaptation transforms
- **Parallel Processing** — rayon-based parallel computation for large image datasets

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-calibrate = "0.2.0"
```

```rust
use oximedia_calibrate::{
    camera::{ColorChecker, ColorCheckerType},
    white::WhiteBalancePreset,
    temp::estimate_color_temperature,
};

// Detect ColorChecker in an image
let checker = ColorChecker::detect_in_image(&image_data, ColorCheckerType::Classic24)?;

// Generate camera profile
let profile = checker.generate_camera_profile()?;

// Apply white balance preset
let balanced = WhiteBalancePreset::Daylight.apply_to_image(&image_data)?;

// Estimate color temperature
let temp = estimate_color_temperature(&image_data)?;
```

## API Overview (54 source files, 493 public items)

**Modules:**
- `camera` — ColorChecker detection and camera profiling (Classic24, Passport)
- `display` — Display calibration and gamma curve measurement
- `icc` — ICC profile parsing, generation, and application (v2/v4)
- `lut` — Calibration LUT creation and verification (1D and 3D)
- `white` — White balance algorithms and presets (gray world, white patch, standard illuminants)
- `temp` — Color temperature estimation and Kelvin-to-RGB conversion
- `gamut` — Gamut mapping strategies (perceptual compression)
- `chromatic` — Chromatic adaptation transforms (Bradford, Von Kries)
- `match_color` — Multi-camera and scene-to-scene color matching

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
