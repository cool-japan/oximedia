# oximedia-timecode

![Status: Stable](https://img.shields.io/badge/status-stable-green)

LTC and VITC timecode reading and writing for OxiMedia. Provides SMPTE 12M compliant timecode support for all standard frame rates with drop frame, user bits, and real-time operation.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **LTC (Linear Timecode)** - Audio-based timecode encoding/decoding
- **VITC (Vertical Interval Timecode)** - Video line-based timecode
- **All Standard Frame Rates** - 23.976, 24, 25, 29.97 DF/NDF, 30, 50, 59.94, 60 fps
- **Drop Frame Support** - Proper SMPTE drop frame arithmetic
- **Non-drop Frame** - Standard non-drop frame timecode
- **User Bits** - 32-bit user data encoding/decoding
- **Burn-in** - Timecode burn-in for video frames
- **Timecode Math** - Add, subtract, compare, and convert timecodes
- **MIDI Timecode (MTC)** - MIDI time code support
- **Drift Detection** - Monitor and correct timecode drift
- **Range Operations** - Timecode range and interval handling
- **Real-time Capable** - Optimized for real-time operations
- **Timecode Continuity** - Detect and handle timecode discontinuities
- **Frame Offset** - Frame-accurate offset arithmetic
- **Timecode Comparison** - Compare and sort timecodes
- **Format Conversion** - Convert between timecode formats
- **Interpolation** - Interpolate timecodes between known points
- **Metadata** - Attach metadata to timecode streams
- **SMPTE Ranges** - Validate against SMPTE timecode ranges
- **Timecode Validation** - Validate timecode values and sequences
- **Sync Mapping** - Map between different timecode streams
- **Calculator Utilities** - High-level timecode calculator API

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-timecode = "0.2.0"
```

```rust
use oximedia_timecode::{Timecode, FrameRate};

// Create a timecode
let tc = Timecode::new(1, 2, 3, 4, FrameRate::Fps25)?;
println!("{}", tc); // "01:02:03:04"

// Convert to frame count
let frames = tc.to_frames();

// Increment by one frame
let mut tc = Timecode::new(0, 0, 0, 24, FrameRate::Fps25)?;
tc.increment()?;
assert_eq!(tc.seconds, 1);
assert_eq!(tc.frames, 0);

// Drop frame timecode (29.97 DF)
let df_tc = Timecode::new(1, 2, 3, 4, FrameRate::Fps2997DF)?;
println!("{}", df_tc); // "01:02:03;04" (semicolon separator)
```

## API Overview

- `Timecode` — SMPTE timecode with HH:MM:SS:FF representation, drop frame support
- `FrameRate` — Fps23976, Fps24, Fps25, Fps2997DF, Fps2997NDF, Fps30, Fps50, Fps5994, Fps60
- `FrameRateInfo` — Frame rate with drop frame flag
- `TimecodeReader` / `TimecodeWriter` — Traits for LTC/VITC I/O
- `TimecodeError` — Invalid hours, minutes, seconds, frames, drop frame, sync, CRC errors
- Modules: `burn_in`, `continuity`, `drop_frame`, `duration`, `frame_offset`, `frame_rate`, `ltc`, `ltc_encoder`, `ltc_parser`, `midi_timecode`, `reader`, `sync`, `sync_map`, `tc_calculator`, `tc_compare`, `tc_convert`, `tc_drift`, `tc_interpolate`, `tc_math`, `tc_metadata`, `tc_range`, `tc_smpte_ranges`, `tc_validator`, `timecode_calculator`, `timecode_format`, `timecode_range`, `vitc`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
