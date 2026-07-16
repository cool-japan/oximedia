# oximedia-colormgmt

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional color management system for OxiMedia, providing ICC profiles, ACES workflow, HDR processing, and accurate color space conversions.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Standard Color Spaces** — sRGB, Adobe RGB, ProPhoto RGB, Display P3, Rec.709, Rec.2020, DCI-P3
- **ACES Support** — Full ACES workflow: IDT, RRT, ODT, LMT with AP0 and AP1 primaries
- **ICC Profile Support** — Parse, validate, and apply ICC v2/v4 profiles (nom-based parsing)
- **HDR Processing** — PQ and HLG transfer functions; `ToneCurve` enum: `ReinhardSimple`, `ReinhardExtended { l_white }`, `FilmicHable` (Hable/Uncharted2 7-param), `AcesFitted` (Narkowicz rational)
- **Gamut Mapping** — Advanced gamut compression and expansion algorithms
- **Color Transforms** — Matrix-based, LUT-based, and parametric transforms
- **Professional Accuracy** — ΔE < 1 for standard conversions, proper linear-light processing
- **Color Appearance Models** — CIECAM02 and related models
- **Color Difference** — ΔE76, ΔE94, ΔE2000 computation
- **Color Blindness Simulation** — Deuteranopia, protanopia, tritanopia
- **Color Harmony** — Complementary, analogous, triadic color schemes
- **Spectral Data** — Spectral locus and observer data
- **Parallel Processing** — rayon-based parallel color transformation

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-colormgmt = "0.2.0"
# Optional features:
oximedia-colormgmt = { version = "0.2.0", features = ["lut-integration", "gpu-accel"] }
```

```rust
use oximedia_colormgmt::{colorspaces::ColorSpace, transforms::rgb_to_rgb};

let srgb = ColorSpace::srgb()?;
let rec2020 = ColorSpace::rec2020()?;
let rgb = [0.5, 0.3, 0.2];
let converted = rgb_to_rgb(&rgb, &srgb, &rec2020);
```

```rust
use oximedia_colormgmt::aces::{AcesColorSpace, AcesTransform};

let transform = AcesTransform::new(AcesColorSpace::ACEScg, AcesColorSpace::ACES2065_1);
let converted = transform.apply([0.5, 0.3, 0.2]);
```

```rust
use oximedia_colormgmt::pipeline::{ColorPipeline, ColorTransform};
use oximedia_colormgmt::colorspaces::ColorSpace;

let srgb = ColorSpace::srgb()?;
let mut pipeline = ColorPipeline::new();
pipeline.add_transform(ColorTransform::Linearize(srgb));
let result = pipeline.transform_pixel([0.5, 0.3, 0.2]);
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `lut-integration` | LUT-based transforms via oximedia-lut (default: enabled) |
| `gpu-accel` | GPU-accelerated color operations via oximedia-gpu (default: enabled) |
| `rayon` | Parallel processing (default: enabled) |

## API Overview (58 source files, 634 public items)

**Core modules:**
- `colorspaces` — Color space definitions and primaries (sRGB, Rec.709, Rec.2020, DCI-P3, etc.)
- `aces`, `aces_pipeline`, `aces_config` — ACES color management workflow
- `icc`, `icc_profile` — ICC profile parsing, generation, and application
- `hdr`, `hdr_color` — HDR transfer functions (PQ, HLG) and tone mapping
- `gamut`, `gamut_mapping`, `gamut_clip` — Gamut management and compression
- `transforms` — Color transform operations (matrix, LUT, parametric)
- `pipeline` — Composable color pipeline
- `chromatic_adapt`, `chromatic_adaptation` — Bradford and Von Kries adaptation
- `tone_map` — Tone mapping operators
- `transfer_function` — Opto-electronic transfer functions
- `color_diff` — ΔE76/ΔE94/ΔE2000 color difference
- `grading` — Color grading tools

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
