# oximedia-vfx

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional video effects library for OxiMedia. Provides production-quality implementations of professional video effects including transitions, generators, keying, time effects, distortion, stylization, light effects, particle systems, and text rendering.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Transitions
- Cross-dissolve with custom curves, 30+ wipe patterns, push/slide/zoom, 3D effects (cube, flip, page curl)

### Generators
- SMPTE/EBU color bars, test patterns (checkerboard, grid, zone plate), noise (white, pink, Perlin), gradients, solid colors

### Keying
- Advanced green/blue screen keying, spill suppression, edge refinement, chroma key

### Time Effects
- Time remapping with custom curves, speed ramping, freeze frame, reverse playback

### Visual Effects
- Lens distortion, barrel/pincushion, wave/ripple distortion, depth of field
- Cartoon/cel-shading, sketch, oil paint, mosaic, halftone stylization
- Lens flare, light rays, glow, HDR bloom
- Snow, rain, sparks, and dust particle systems
- Motion blur, chromatic aberration, film grain, fog, heat distortion

### Compositing and Color
- Multi-layer compositing with blend modes
- Color grading pipeline with LUT application
- Edge detection filters (Sobel, Prewitt, Laplacian, Roberts)
- Deformable mesh warping
- Vector blur

### Text and Shapes
- High-quality text rendering with typewriter/fade/slide animation
- Shape drawing with keyframe animation and animated masks

### Particle and Trail Effects
- Configurable particle simulation with physics
- Trail effects for motion visualization
- Noise-field driven particle movement

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-vfx = "0.2.0"
```

```rust
use oximedia_vfx::{VideoEffect, Frame, EffectParams, QualityMode};

let mut frame = Frame::new(1920, 1080)?;
frame.clear([0, 0, 0, 255]);

let params = EffectParams::new()
    .with_progress(0.5)
    .with_quality(QualityMode::Final);
```

## API Overview

- `VideoEffect` — Core trait for single-input effects: apply(), reset(), supports_gpu()
- `TransitionEffect` — Core trait for two-input transitions: apply(), reset()
- `Frame` — RGBA frame with pixel access: new(), from_data(), get_pixel(), set_pixel(), clear()
- `EffectParams` — Progress, quality, time, GPU, and motion blur parameters
- `QualityMode` — Draft, Preview, Final
- `ParameterTrack` / `Keyframe` / `EasingFunction` — Keyframe animation with Linear, EaseIn, EaseOut, EaseInOut, Bezier
- `Color` / `Rect` / `Vec2` — Common graphics primitives
- `VfxError` / `VfxResult` — Error and result types
- Modules: `blur_kernel`, `chroma_key`, `chromatic_aberration`, `color_grade`, `color_grading`, `color_lut`, `compositing`, `deform_mesh`, `depth_of_field`, `distortion`, `edge_detect`, `film_effect`, `fog`, `generator`, `grade_pipeline`, `heat_distort`, `keying`, `lens_aberration`, `lens_flare`, `light`, `mblur_config`, `motion_blur`, `noise_field`, `particle`, `particle_fx`, `particle_sim`, `presets`, `render_pass`, `ripple`, `rotoscoping`, `shape`, `style`, `text`, `time`, `tracking`, `trail_effect`, `transition`, `utils`, `vector_blur`, `vfx_preset`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
