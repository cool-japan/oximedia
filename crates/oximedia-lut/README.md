# oximedia-lut

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 550](https://img.shields.io/badge/tests-550-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

Professional LUT (Look-Up Table) and color science library for OxiMedia, providing 1D/3D LUT operations, ACES workflow, gamut mapping, and tone mapping.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **1D LUTs** — Per-channel curves with linear/cubic interpolation
- **3D LUTs** — Full RGB transforms with trilinear/tetrahedral interpolation
- **LUT Formats** — `.cube` (Adobe/DaVinci), `.3dl` (Autodesk), `.csp` (Cinespace)
- **LUT Composition** — Chain multiple LUTs together with `lut_chain`
- **LUT Combining** — Bake/combine multiple LUTs into one
- **LUT Inversion** — Generate inverse LUTs
- **LUT Analysis** — Validate and analyze LUT properties
- **LUT Resampling** — Resample LUT to different grid sizes
- **LUT Dithering** — Dither LUT output for bit depth reduction
- **LUT Statistics** — Statistical analysis of LUT properties
- **LUT Metadata** — Metadata embedding in LUT files
- **LUT Provenance** — Track LUT origin and history
- **LUT Versioning** — LUT version management
- **LUT Fingerprint** — Perceptual LUT fingerprinting
- **LUT Gradient** — Gradient-based LUT analysis
- **Color Spaces** — Rec.709, Rec.2020, DCI-P3, Adobe RGB, sRGB, ProPhoto RGB, ACES AP0/AP1
- **Gamut Mapping** — Soft-clip, desaturate, and roll-off algorithms
- **Gamut Compression** — Gamut compression LUT generation
- **Tone Mapping** — Reinhard, ACES, Hable (Uncharted 2) operators
- **ACES Workflow** — Full ACES color management with RRT and ODT
- **Chromatic Adaptation** — Bradford and Von Kries transforms
- **Color Temperature** — Kelvin (2000K–11000K) to RGB conversion
- **HDR Pipeline** — HDR LUT generation and application
- **HDR Metadata** — HDR metadata for LUT delivery
- **Domain Clamping** — Configurable input/output domain clamp
- **Tetrahedral Interpolation** — Industry-standard tetrahedral interpolation

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-lut = "0.2.0"
```

```rust
use oximedia_lut::{Lut3d, LutInterpolation, ColorSpace};

fn example() -> Result<(), Box<dyn std::error::Error>> {
    // Load a 3D LUT from a .cube file
    let lut = Lut3d::from_file("colorgrade.cube")?;

    // Apply the LUT to an RGB pixel
    let input = [0.5, 0.3, 0.7];
    let output = lut.apply(&input, LutInterpolation::Tetrahedral);

    // Convert between color spaces
    let rec709_rgb = [0.8, 0.2, 0.4];
    let rec2020_rgb = ColorSpace::Rec709.convert(ColorSpace::Rec2020, &rec709_rgb)?;
    Ok(())
}
```

## API Overview

**Core types:**
- `Lut3d` — 3D LUT with trilinear/tetrahedral interpolation
- `Lut1d` — 1D per-channel LUT
- `LutInterpolation` — Trilinear / Tetrahedral / Cubic interpolation
- `ColorSpace` — Color space enum (Rec709, Rec2020, DCI-P3, ACES, sRGB, ...)

**LUT type modules:**
- `lut1d` — 1D LUT implementation
- `lut3d` — 3D LUT implementation
- `color_cube` — Color cube data structure
- `identity_lut` — Identity LUT generation

**Interpolation:**
- `interpolation` — Interpolation algorithms
- `tetrahedral` — Tetrahedral interpolation (industry standard)
- `lut_interpolation` — LUT-specific interpolation

**File formats:**
- `formats` — Format dispatcher
- `formats::cube` — Adobe/DaVinci `.cube` format
- `formats::threedl` — Autodesk `.3dl` format
- `formats::csp` — Cinespace `.csp` format
- `cube_writer` — `.cube` file writer
- `lut_io` — LUT file I/O
- `export` — LUT export utilities

**Color science:**
- `colorspace` — Color space definitions and transforms
- `chromatic` — Chromatic adaptation (Bradford/Von Kries)
- `temperature` — Color temperature to RGB
- `matrix` — Color matrix operations

**ACES and tone mapping:**
- `aces` — ACES color management (RRT, ODT)
- `tonemap` — Tone mapping operators (Reinhard, ACES, Hable)

**Gamut:**
- `gamut` — Gamut mapping algorithms
- `gamut_compress_lut` — Gamut compression LUT

**HDR:**
- `hdr_lut` — HDR LUT generation
- `hdr_pipeline` — HDR processing pipeline
- `hdr_metadata` — HDR metadata types

**LUT construction and analysis:**
- `builder` — LUT construction builder
- `baking` — LUT baking (compositing operations into one LUT)
- `lut_chain` — Chain multiple LUTs
- `lut_combine` — Combine/merge LUTs
- `lut_analysis` — LUT property analysis
- `lut_validate` — LUT validation
- `lut_stats` — LUT statistics
- `lut_resample` — LUT grid resampling
- `lut_dither` — Output dithering
- `domain_clamp` — Input/output domain clamping

**Metadata and identity:**
- `lut_metadata` — LUT metadata embedding
- `lut_provenance` — Provenance tracking
- `lut_version` — Version management
- `lut_fingerprint` — Perceptual fingerprinting
- `lut_gradient` — Gradient analysis

**Preview:**
- `preview` — LUT preview rendering

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
