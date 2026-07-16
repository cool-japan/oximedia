# oximedia-captions

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional closed captioning and subtitle authoring system for OxiMedia, supporting all major broadcast, web, and professional formats.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Closed Caption Formats** — CEA-608 (Line 21/NTSC), CEA-708 (ATSC), Teletext (EBU/BBC), ARIB (Japan)
- **Subtitle Formats** — SRT, WebVTT, ASS/SSA, TTML, DFXP, SCC, STL (EBU/Spruce), iTunes Timed Text
- **Embedded Formats** — MPEG-TS DVB, MP4 608/708, Matroska/WebM, Blu-ray PGS, DVD VobSub
- Caption authoring, editing, and frame-accurate timing
- Style and positioning control
- FCC and WCAG compliance validation
- Multi-language support and translation workflow
- Quality control and reporting
- Template system
- Import/export between all supported formats
- Live captioning support
- Speaker diarization
- ASR (Automatic Speech Recognition) integration
- Unicode and multi-byte encoding support (encoding_rs)
- Language detection (whatlang)
- TTML/DFXP XML parsing (quick-xml)

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-captions = "0.2.0"
# or with specific format support:
oximedia-captions = { version = "0.2.0", features = ["cea", "web", "broadcast"] }
```

```rust
use oximedia_captions::{Caption, CaptionTrack, CaptionFormat, CaptionStyle};

let mut track = CaptionTrack::new();
let caption = Caption::new(
    "Hello, world!",
    Timestamp::from_secs(1.0),
    Timestamp::from_secs(3.0),
);
track.add(caption);
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `cea` | CEA-608/708 closed caption support |
| `broadcast` | Teletext, ARIB, DVB formats |
| `web` | WebVTT, TTML, DFXP formats |
| `professional` | SCC, STL, iTunes Timed Text |
| `all-formats` | All of the above (default) |

## API Overview (63 source files, 843 public items)

**Core types:**
- `Caption` — Individual caption entry with timing and text
- `CaptionTrack` — Collection of captions
- `CaptionFormat` — Format identifier enum
- `CaptionStyle`, `CaptionId`, `Timestamp`, `Language` — Supporting types

**Modules:**
- `formats` — Format-specific parsers and writers (CEA-608/708, SRT, WebVTT, TTML, ASS, STL, SCC)
- `authoring` — Caption authoring and editing tools
- `validation`, `caption_validator` — FCC and WCAG compliance validation
- `caption_qc` — Caption quality control
- `caption_renderer` — Visual rendering of caption overlays
- `translation` — Translation workflow integration
- `live_caption` — Real-time live captioning support
- `speaker_diarization` — Speaker identification and labeling
- `asr` — Automatic Speech Recognition integration

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
