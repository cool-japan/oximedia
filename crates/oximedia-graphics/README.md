# oximedia-graphics

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.1](https://img.shields.io/badge/version-0.2.1-blue)
![Tests: 1014](https://img.shields.io/badge/tests-1014-brightgreen)
![Updated: 2026-08-12](https://img.shields.io/badge/updated-2026--08--12-blue)

Broadcast graphics engine for OxiMedia, providing 2D vector graphics, advanced typography, broadcast graphics elements, keyframe animation, and GPU-accelerated rendering.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **2D Vector Graphics** — SVG-compatible vector rendering via tiny-skia
- **Advanced Typography** — Font rendering, text layout, Unicode support via fontdue/ab_glyph
- **Broadcast Elements** — Lower thirds, tickers, bugs, scoreboards, virtual sets, weather widgets, clock/countdown timers
- **Keyframe Animation** — Full animation system with curve interpolation
- **Template System** — Tera-based template engine for dynamic graphics
- **Real-time Overlay** — Video overlay and compositing
- **GPU Acceleration** — WGPU-based GPU rendering (feature-gated)
- **Control Server** — HTTP/WebSocket control API for live graphics (feature-gated)
- **Particle System** — Particle effects engine
- **Sprite Sheets** — Sprite sheet loading and animation
- **Color Picker** — Color selection widget
- **Layout Engine** — Flexible CSS-like layout system
- **Gradient Fill** — Linear and radial gradient fills
- **Mask Layer** — Alpha masking and compositing
- **Path Builder** — Vector path construction API
- **Transition Wipe** — Wipe transition effects
- **Shape Rendering** — Parametric shape rendering
- **Text Layout** — Multi-line text layout engine

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-graphics = "0.2.0"
# With GPU and server features (default):
oximedia-graphics = { version = "0.2.0", features = ["gpu", "server"] }
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `gpu` | WGPU-based GPU rendering |
| `server` | HTTP/WebSocket control server via axum |

## API Overview

**Core types:**
- `GraphicsError`, `Result` — Error types
- `VERSION` — Package version constant

**Broadcast element modules:**
- `lower_third` — Lower third graphics
- `ticker` — News/information ticker
- `scoreboard` — Sports scoreboard
- `weather_widget` — Weather display widget
- `clock_widget` — Clock display widget
- `countdown_timer` — Countdown timer

**Rendering modules:**
- `render` — Core rendering engine
- `text`, `text_renderer`, `text_layout` — Text rendering and layout
- `svg_renderer` — SVG rendering
- `primitives` — Geometric primitives
- `shape_render` — Parametric shape rendering
- `overlay` — Video overlay compositing

**Animation modules:**
- `animation`, `animation_curve` — Keyframe animation system
- `keyframe` — Keyframe management
- `transitions` — Animated transitions
- `transition_wipe` — Wipe transition effects

**Template and preset modules:**
- `template`, `graphic_template` — Template system
- `elements` — Pre-built graphic elements
- `presets` — Preset graphic configurations
- `professional` — Broadcast-specific professional features

**Other rendering modules:**
- `particles` — Particle effects engine
- `effects` — Visual effects
- `virtual_set` — Virtual set integration
- `sprite_sheet` — Sprite sheet animation

**Color and layout:**
- `color`, `color_blend` — Color management and blending
- `color_picker` — Color selection widget
- `layout_engine` — Flexible layout system
- `gradient_fill` — Gradient fill rendering
- `mask_layer` — Alpha mask layers
- `path_builder` — Vector path construction

**Typography:**
- `font_metrics` — Font measurement and metrics
- `bitmap_font` — Bitmap font rendering

**Feature-gated modules:**
- `control` — HTTP/WebSocket control server (requires `server`)
- `gpu` — GPU rendering (requires `gpu`)

## Examples

```toml
[dependencies]
oximedia-graphics = { version = "0.2.0", features = ["gpu"] }
```

See the `examples/` directory for broadcast graphics usage examples.

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
