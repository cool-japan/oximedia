# oximedia-scaling

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional video scaling operations for OxiMedia. Provides high-quality video scaling with bilinear, bicubic, and Lanczos filtering, aspect ratio preservation, super-resolution, thumbnail generation, and more.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 | Tests: extensively tested — 2026-07-14

## Features

- **Bilinear interpolation** - Fast scaling with good quality
- **Bicubic interpolation** - Higher quality scaling
- **Lanczos filtering** - Highest quality downscaling with minimal ringing
- **Aspect ratio preservation** - Letterbox, crop, and stretch modes
- **Super-resolution** - AI-assisted upscaling
- **Adaptive scaling** - Content-aware scaling algorithms
- **Chroma scaling** - Proper chroma plane scaling for YUV formats
- **Thumbnail generation** - Efficient thumbnail creation
- **Crop and pad** - Crop/pad with scaling pipeline
- **Resolution ladder** - Multi-resolution output generation
- **ROI scaling** - Region-of-interest scaling
- **Field scaling** - Interlaced content support
- **Deinterlacing** - Convert interlaced to progressive
- **Scale pipeline** - Composable scaling filter chains
- **Sharpness scaling** - Sharpness-preserving scaling
- **Quality metrics** - PSNR/SSIM quality measurement for scale operations
- **Tile scaling** - Tile-based scaling for large images

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-scaling = "0.2.0"
```

```rust
use oximedia_scaling::{VideoScaler, ScalingParams, ScalingMode, AspectRatioMode};

// Create a scaler for 1080p output
let params = ScalingParams::new(1920, 1080)
    .with_mode(ScalingMode::Lanczos)
    .with_aspect_ratio(AspectRatioMode::Letterbox);

let scaler = VideoScaler::new(params);

// Calculate output dimensions preserving aspect ratio
let (out_w, out_h) = scaler.calculate_dimensions(3840, 2160);
```

## API Overview

- `VideoScaler` — Main scaler with dimension calculation
- `ScalingParams` — Target dimensions, mode, and aspect ratio configuration
- `ScalingMode` — Bilinear, Bicubic, Lanczos
- `AspectRatioMode` — Stretch, Letterbox, Crop
- Modules: `adaptive_scaling`, `aspect_preserve`, `aspect_ratio`, `bicubic`, `chroma_scale`, `crop`, `crop_scale`, `deinterlace`, `field_scale`, `lanczos`, `pad`, `pad_scale`, `quality_metric`, `quality_metrics`, `resampler`, `resolution_ladder`, `roi_scale`, `scale_config`, `scale_filter`, `scale_pipeline`, `sharpness_scale`, `super_resolution`, `thumbnail`, `tile`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
