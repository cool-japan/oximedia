# oximedia-effects

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Professional audio and video effects suite for OxiMedia, providing production-quality implementations of reverb, delay, modulation, distortion, dynamics, filters, pitch/time, vocoding, and video effects.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Reverb** — Room reverb (Schroeder architecture), Hall reverb (plate/convolution)
- **Delay/Echo** — Simple delay, Tape echo, Multi-tap Delay, Ping-pong Delay
- **Modulation** — Chorus, Flanger, Tremolo, Vibrato, Ring Modulator, Auto Pan
- **Distortion** — Saturation (soft clip), Overdrive, Waveshaper, Bit Crusher
- **Dynamics** — Noise Gate, Compressor, Limiter, Expander, De-esser, Ducking
- **Filters** — Biquad IIR (LP, HP, BP, notch, shelving), Filter Bank, EQ (parametric)
- **Pitch/Time** — Pitch Shifter, Time Stretch, Harmonizer with formant preservation
- **Vocoding** — Channel Vocoder, Auto-tune pitch correction
- **Glitch** — Buffer glitch and stutter effects
- **Stereo Widener** — Stereo width enhancement
- **Transient Shaper** — Attack/sustain transient control
- **Video Effects** — Barrel lens distortion, Blend modes, Color grading, Composite, Chroma key
- **Video Grain** — Film grain synthesis
- **Lens Flare** — Lens flare synthesis
- **Vignette** — Vignette overlay
- **Chromatic Aberration** — Color fringing effect
- **Motion Blur** — Motion blur synthesis
- **Warp** — Video warp/distortion
- Real-time capable with no allocations in process loops
- Sample-accurate parameter smoothing
- No unsafe code

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-effects = "0.2.0"
```

```rust
use oximedia_effects::{AudioEffect, reverb::Freeverb, ReverbConfig};

let config = ReverbConfig::default()
    .with_room_size(0.8)
    .with_damping(0.5)
    .with_wet(0.3);

let mut reverb = Freeverb::new(config, 48000.0);

let mut left = vec![0.0; 1024];
let mut right = vec![0.0; 1024];
reverb.process_stereo(&mut left, &mut right);
```

## API Overview

**Core trait:**
- `AudioEffect` — Unified interface for all audio effects

**Reverb and delay:**
- `reverb` — Generic reverb interface
- `reverb_hall` — Hall reverb implementation
- `room_reverb` — Room reverb (Freeverb/Schroeder)
- `delay`, `delay_line` — Delay and echo effects
- `tape_echo` — Tape echo simulation

**Modulation:**
- `chorus` — Chorus effect
- `flanger` — Flanger effect
- `tremolo` — Tremolo (amplitude modulation)
- `vibrato` — Vibrato (pitch modulation)
- `auto_pan` — Automatic stereo panning
- `modulation` — General modulation utilities

**Distortion and saturation:**
- `distort`, `distortion` — Distortion/overdrive
- `saturation` — Saturation/soft clip
- `waveshaper` — Waveshaper transfer function

**Dynamics:**
- `compressor`, `compressor_look` — Dynamic compressor with lookahead
- `dynamics` — General dynamics processing
- `deesser` — De-essing
- `ducking` — Sidechain ducking

**Filters and EQ:**
- `eq` — Parametric equalizer
- `filter`, `filter_bank` — Biquad filters and filter bank

**Pitch and time:**
- `pitch` — Pitch shifting
- `time_stretch` — Time stretching
- `ring_mod` — Ring modulator

**Other audio:**
- `glitch` — Glitch/stutter effects
- `stereo_widener` — Stereo width enhancement
- `transient_shaper` — Transient shaping
- `spatial_audio` — Spatial/3D audio processing
- `vocoder` — Channel vocoder
- `utils` — Shared effect utilities

**Video effect modules:**
- `video::blend` — Blend mode operations
- `video::chromakey` — Chroma keying
- `video::chromatic_aberration` — Chromatic aberration
- `video::color_grade` — Color grading
- `video::grain` — Film grain synthesis
- `video::lens_flare` — Lens flare
- `video::motion_blur` — Motion blur
- `video::vignette` — Vignette overlay
- `video::mod` — Video effect coordination

**Other modules:**
- `blend` — Blend utilities
- `composite` — Video compositing
- `barrel_lens` — Lens distortion correction
- `luma_key` — Luminance keying
- `keying` — Keying utilities
- `warp` — Video warp/distortion

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
