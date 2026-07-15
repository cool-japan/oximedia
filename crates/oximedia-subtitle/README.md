# oximedia-subtitle

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Comprehensive subtitle and closed caption rendering for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Multiple Subtitle Formats**
  - SubRip (SRT) - Simple text subtitles
  - WebVTT - Web Video Text Tracks with positioning
  - SSA/ASS - Advanced SubStation Alpha with full styling
  - CEA-608/708 - Closed captions

- **Text Rendering**
  - TrueType/OpenType font support via fontdue
  - Glyph caching for performance
  - Full Unicode support including emoji
  - Bidirectional text (RTL languages like Arabic, Hebrew)

- **Styling**
  - Font properties: size, weight, style
  - Colors: text, outline, shadow
  - Positioning: top, middle, bottom, custom
  - Alignment: left, center, right
  - Outline and shadow effects

- **Advanced Features**
  - Burn-in onto video frames
  - Word wrapping and line breaking
  - Fade in/out animations
  - Karaoke effects (SSA/ASS)
  - Dynamic positioning
  - Accessibility features
  - Translation/localization support
  - Spell checking
  - Timing adjustment and readability optimization
  - Overlap detection and resolution
  - Cue point annotations
  - Multi-format export
  - Subtitle merging and diffing
  - Sanitization and validation
  - Full-text search across subtitles
  - Statistics and analytics
  - Subtitle segmentation

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-subtitle = "0.2.0"
```

```rust
use oximedia_subtitle::{SrtParser, WebVttParser, AssParser};

// Parse SRT subtitles
let text = std::fs::read_to_string("movie.srt")?;
let subtitles = SrtParser::parse(&text)?;

// Parse WebVTT
let vtt = std::fs::read_to_string("movie.vtt")?;
let vtt_subs = WebVttParser::parse(&vtt)?;

// Parse ASS
let ass = std::fs::read_to_string("movie.ass")?;
let ass_subs = AssParser::parse(&ass)?;

// Check active subtitle at timestamp
for sub in &subtitles {
    if sub.is_active(2000) {
        println!("Active: {}", sub.text);
    }
}
```

## API Overview

- `SubtitleRenderer` — Render subtitles onto video frames
- `Subtitle` — Single subtitle cue with timing, text, style, position, animations
- `SrtParser` / `WebVttParser` / `AssParser` — High-level format parsers
- `SubtitleStyle` — Font, color, outline, shadow, position, alignment
- `Font` / `GlyphCache` — Font loading and glyph caching
- `TextLayout` / `TextLayoutEngine` — Bidirectional text layout
- `Color` / `OutlineStyle` / `ShadowStyle` / `Position` / `Alignment` — Style primitives
- `Animation` — FadeIn, FadeOut, and other animation types
- `SubtitleError` / `SubtitleResult` — Error and result types
- `overlay_subtitle()` — Overlay subtitle onto raw frame data
- Modules: `accessibility`, `burn_in`, `cea`, `convert`, `cue_parser`, `cue_point`, `cue_timing`, `error`, `font`, `format_convert`, `line_break`, `overlap_detect`, `overlay`, `parser`, `position_calc`, `reading_speed`, `renderer`, `segmentation`, `spell_check`, `style`, `sub_style`, `subtitle_diff`, `subtitle_export`, `subtitle_index`, `subtitle_merge`, `subtitle_sanitize`, `subtitle_search`, `subtitle_stats`, `subtitle_style_ext`, `subtitle_validator`, `text`, `timing`, `timing_adjust`, `translation`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
