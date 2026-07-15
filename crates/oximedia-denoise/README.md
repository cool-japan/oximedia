# oximedia-denoise

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Professional video and audio denoising for OxiMedia, with spatial, temporal, hybrid, and frequency-domain algorithms.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Spatial Denoising** — Bilateral filtering (edge-preserving), Non-Local Means (patch-based), Wiener filtering, wavelet denoising
- **Temporal Denoising** — Temporal averaging, temporal median, motion-compensated filtering, Kalman filtering
- **Hybrid Denoising** — Spatio-temporal filtering, adaptive content-aware denoising
- **Motion Estimation** — Block-based motion estimation for temporal compensation
- **Film Grain** — Grain analysis, preservation, and synthesis
- **Multi-scale Processing** — Image pyramid and wavelet multi-resolution decomposition
- **Automatic Noise Estimation** — Blind noise level estimation and noise model fitting
- **Audio Denoising** — Spectral gating and audio noise reduction
- **Chroma Denoising** — Dedicated chroma channel denoising
- **Deblocking** — Compression artifact removal
- **Region-based Denoising** — Apply different denoise strengths per image region
- **Adaptive Denoising** — Content-aware strength adaptation
- **Denoise Metrics** — PSNR, SSIM quality measurement
- **Denoise Profiles** — Preset profiles for film, video, web, and more

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-denoise = "0.2.0"
```

```rust
use oximedia_denoise::{DenoiseConfig, DenoiseMode, Denoiser};

let config = DenoiseConfig {
    mode: DenoiseMode::Balanced,
    strength: 0.7,
    temporal_window: 5,
    preserve_edges: true,
    preserve_grain: false,
};

let mut denoiser = Denoiser::new(config);
// denoiser.process(&frame) to denoise a video frame
```

## API Overview

**Core types:**
- `Denoiser` — Main denoising engine
- `DenoiseConfig` — Configuration: mode, strength, temporal window
- `DenoiseMode` — Fast / Balanced / Quality / GrainAware / Custom
- `DenoiseError`, `DenoiseResult` — Error types

**Spatial modules:**
- `spatial` — Spatial denoising algorithms (bilateral, NLM, Wiener)
- `bilateral` — Bilateral edge-preserving filter
- `chroma_denoise` — Chroma-specific denoising
- `deblock` — Deblocking / artifact removal
- `region_denoise` — Region-based variable-strength denoising

**Temporal modules:**
- `temporal` — Temporal denoising (averaging, median, Kalman)
- `motion` — Motion estimation for temporal compensation

**Hybrid and multi-scale:**
- `hybrid` — Combined spatio-temporal algorithms
- `multiscale` — Multi-scale pyramid processing
- `adaptive_denoise` — Adaptive content-aware denoising

**Noise analysis:**
- `noise_estimate` — Blind noise level estimation
- `noise_model` — Noise model definitions
- `estimator` — Statistical noise estimator
- `profile` — Denoise presets/profiles

**Audio modules:**
- `audio`, `audio_denoise` — Audio noise reduction
- `spectral_gate` — Spectral gating for audio
- `video`, `video_denoise` — Video-specific denoising pipelines

**Quality:**
- `grain` — Film grain analysis, preservation, and synthesis
- `denoise_metrics` — Quality metrics (PSNR, SSIM)
- `denoise_config` — Configuration types

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
