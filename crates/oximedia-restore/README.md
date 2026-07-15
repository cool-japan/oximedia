# oximedia-restore

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Professional audio and video restoration tools for OxiMedia. Provides comprehensive restoration capabilities for recovering and enhancing degraded recordings, including audio restoration and video artifact removal.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

### Audio Restoration
- **Click/Pop Removal** - Remove vinyl clicks and digital glitches
- **Hum Removal** - Remove 50Hz/60Hz hum and harmonics
- **Noise Reduction** - Spectral subtraction, noise gate, and Wiener filtering
- **Declipping** - Restore clipped audio peaks
- **Dehiss** - Remove tape hiss and background noise
- **Decrackle** - Remove crackle from old recordings
- **Azimuth Correction** - Correct tape azimuth errors
- **Wow/Flutter Removal** - Remove tape speed variations
- **DC Offset Removal** - Remove DC bias
- **Phase Correction** - Correct phase issues
- **Flutter Repair** - Repair flutter-induced pitch variation
- **Room Correction** - Room acoustics correction
- **Pitch Correction** - Pitch correction and auto-tune
- **Noise Profile Matching** - Match and subtract noise profiles
- **Stereo Field Repair** - Repair stereo field issues
- **Spectral Repair** - Frequency-domain repair

### Video Restoration
- **Banding Reduction** - Remove color banding artifacts
- **Color Bleed** - Fix color bleeding from composite video
- **Color Restoration** - Restore faded colors
- **Deband** - Remove banding in compressed video
- **Deflicker** - Remove temporal flickering
- **Dropout Fix** - Correct tape dropouts
- **Film Grain** - Film grain generation and management
- **Grain Add/Restore** - Add synthetic or restore original grain
- **Scan Line Repair** - Fix damaged scan lines
- **Spectral Repair** - Frequency-domain video restoration
- **Telecine Detection** - Detect and correct telecine pulldown
- **Upscaling** - AI-assisted upscaling
- **Vintage Effects** - Vintage look application

### Restoration Presets
- **Vinyl Restoration** - Click removal, decrackle, hum removal
- **Tape Restoration** - Azimuth, wow/flutter, hiss removal
- **Broadcast Cleanup** - Declipping, noise reduction, DC removal
- **Archival** - Full restoration chain for preservation

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-restore = "0.2.0"
```

```rust
use oximedia_restore::{RestoreChain, RestorationStep, presets::VinylRestoration};
use oximedia_restore::{click::{ClickDetector, ClickRemover}, dc::DcRemover};

// Create a restoration chain for vinyl
let mut chain = RestoreChain::new();
chain.add_preset(VinylRestoration::default());

// Process samples
let samples = vec![0.0f32; 44100];
let restored = chain.process(&samples, 44100)?;

// Or process stereo
let (left_out, right_out) = chain.process_stereo(&left, &right, 44100)?;
```

## API Overview

**Core types:**
- `RestoreChain` — Processing chain with ordered restoration steps
- `RestorationStep` — Individual restoration operations:
  - `DcRemoval`, `ClickRemoval`, `HumRemoval`, `NoiseReduction`
  - `WienerFilter`, `NoiseGate`, `Declipping`, `HissRemoval`
  - `CrackleRemoval`, `AzimuthCorrection`, `WowFlutterCorrection`, `PhaseCorrection`

**Audio modules:**
- `azimuth` — Azimuth correction
- `click` — Click/pop detection and removal
- `clip`, `declip` — Clipping detection and restoration
- `crackle` — Crackle detection and removal
- `dc` — DC offset removal
- `flutter_repair` — Flutter repair
- `hiss` — Hiss detection and removal
- `hum` — Hum detection and removal
- `noise` — Noise gate, spectral subtraction, Wiener filter
- `phase` — Phase correction
- `room_correction` — Room acoustics correction
- `wow` — Wow/flutter detection and correction
- `pitch_correct` — Pitch correction
- `noise_profile_match` — Noise profile matching
- `spectral_repair` — Spectral repair
- `stereo_field_repair` — Stereo field repair

**Video modules:**
- `banding_reduce` — Color banding reduction
- `color_bleed` — Color bleed correction
- `color_restore` — Color restoration
- `deband` — Debanding
- `deflicker` — Deflicker
- `dropout_fix` — Dropout correction
- `film_grain` — Film grain management
- `grain_add`, `grain_restore` — Grain synthesis and restoration
- `scan_line` — Scan line repair
- `telecine_detect` — Telecine detection
- `upscale` — AI upscaling
- `vintage` — Vintage effects

**Utilities:**
- `error` — Error types
- `presets` — Restoration presets (Vinyl, Tape, Broadcast, Archival)
- `restore_plan` — Restoration plan management
- `restore_report` — Restoration reporting
- `utils` — Utility functions (interpolation, spectral)

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
