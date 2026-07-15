# oximedia-scopes

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional video scopes (waveform, vectorscope, histogram, parade) for OxiMedia. Provides industry-standard broadcast-quality video scopes for analyzing video signals, ITU-R BT.709/BT.2020 compliant and suitable for broadcast workflows.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 | Tests: extensively tested — 2026-07-14

## Features

- **Waveform Monitor** - Luma, RGB parade, RGB overlay, YCbCr waveform with graticule
- **Vectorscope** - YUV vectorscope with SMPTE color bars, skin tone line, and gamut warnings
- **Histogram** - RGB and luma histograms with statistical overlays
- **Parade** - RGB and YCbCr parade displays with component selection
- **False Color** - Exposure visualization with IRE-based false color mapping
- **CIE Diagram** - CIE 1931 chromaticity diagram
- **HDR Waveform** - HDR waveform with PQ/HLG/nits scale
- **Focus Assist** - Edge peaking for focus confirmation
- **Audio Scope** - Audio level display
- **Zebra** - Over-exposure zebra pattern
- **Lissajous** - Stereo audio phase scope
- **Loudness Scope** - EBU R128 loudness display
- **Gamut Scope** - Gamut boundary visualization
- **Motion Vector Scope** - Motion vector display
- **Exposure Meter** - Incident and reflected exposure metering
- **Peaking** - High-frequency peaking for focus assist
- **Signal Statistics** - Detailed signal statistical analysis
- **Histogram Statistics** - Statistical analysis of histogram data
- **Compliance Checking** - Broadcast legal level compliance verification
- **Color Temperature** - Color temperature measurement display
- **Vectorscope Targets** - SMPTE color bar target overlays

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-scopes = "0.2.0"
```

```rust
use oximedia_scopes::{VideoScopes, ScopeType, ScopeConfig};

// Create video scopes analyzer
let scopes = VideoScopes::new(ScopeConfig::default());

// Analyze frame and generate waveform
let frame_data: Vec<u8> = vec![0u8; 1920 * 1080 * 3]; // RGB24
let waveform = scopes.analyze(&frame_data, 1920, 1080, ScopeType::WaveformLuma)?;

// Render scope to RGBA image
let image = scopes.render(&waveform)?;
```

## API Overview

- `VideoScopes` — Main scope analyzer and renderer
- `ScopeConfig` — Display size, graticule, labels, anti-aliasing, gamut colorspace
- `ScopeType` — WaveformLuma, WaveformRgbParade, WaveformRgbOverlay, WaveformYcbcr, Vectorscope, HistogramRgb, HistogramLuma, ParadeRgb, ParadeYcbcr, FalseColor, CieDiagram, FocusAssist, HdrWaveform
- `ScopeData` — Rendered scope pixel data (RGBA)
- `WaveformMode` / `VectorscopeMode` / `HistogramMode` — Display mode variants
- `GamutColorspace` — Rec709, Rec2020, DciP3
- Modules: `audio_scope`, `cie`, `color_temperature`, `compliance`, `exposure_meter`, `false_color`, `false_color_mapping`, `focus`, `focus_assist`, `gamut_scope`, `hdr`, `histogram`, `histogram_stats`, `lissajous`, `loudness_scope`, `motion_vector_scope`, `overlay`, `parade`, `peaking`, `render`, `scope_layout`, `signal_stats`, `stats`, `vectorscope`, `vectorscope_targets`, `waveform`, `waveform_analyzer`, `zebra`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
