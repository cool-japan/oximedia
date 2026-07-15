# oximedia-mixer

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Professional audio mixer with automation for OxiMedia, providing a full digital audio mixing console with 100+ channels, comprehensive effects, and full parameter automation.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Multi-channel Mixing** — 100+ channels with flexible routing
- **Channel Types** — Mono, Stereo, 5.1, 7.1, and Ambisonics
- **Effect Processing** — Dynamics, EQ, reverb, delay, modulation, distortion
- **Automation System** — Read, Write, Touch, Latch, Trim automation modes
- **Bus Architecture** — Master, group, and auxiliary buses
- **Professional Metering** — Peak, RMS, VU, LUFS, phase correlation
- **Session Management** — Save/load mixer state with undo/redo
- **Channel Strip** — Input gain, phase inversion, insert effects, fader, pan, sends
- **Flexible Routing** — Pre/post-fader sends, direct outs, matrix buses
- **Channel Linking** — Stereo pair linking
- **Real-time Performance** — Lock-free audio path, SIMD DSP, target < 10ms latency at 48kHz/512 samples
- **Zero-copy Routing** — Minimal buffer copies in audio path
- **VCA Groups** — VCA fader grouping
- **Sidechain Support** — Sidechain routing for dynamics processing
- **Scene Recall** — Mixer scene/snapshot management

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-mixer = "0.2.0"
```

```rust
use oximedia_mixer::{AudioMixer, MixerConfig, ChannelType};
use oximedia_audio::ChannelLayout;

let config = MixerConfig {
    sample_rate: 48000,
    buffer_size: 512,
    max_channels: 64,
    ..Default::default()
};

let mut mixer = AudioMixer::new(config);

let channel_id = mixer.add_channel(
    "Vocals".to_string(),
    ChannelType::Stereo,
    ChannelLayout::Stereo,
)?;
```

## API Overview

**Core types:**
- `AudioMixer` — Main digital mixing console
- `MixerConfig` — Configuration: sample rate, buffer size, max channels
- `ChannelType` — Mono, Stereo, Surround5_1, Surround7_1, Ambisonics

**Architecture components:**
- Channel strip: input gain, phase, inserts, fader, pan, sends
- Bus types: Master, Group, Auxiliary, Matrix
- Automation: Read, Write, Touch, Latch, Trim modes
- Metering: Peak, RMS, VU (IEC 60268-10), LUFS (EBU R128), phase correlation

**Modules:**
- `automation`, `automation_lane` — Parameter automation
- `aux_send` — Auxiliary send routing
- `bus`, `group_bus` — Bus architecture
- `channel`, `channel_strip` — Channel strip processing
- `crossfade` — Crossfade transitions
- `delay_line` — Digital delay lines
- `dynamics` — Dynamics processing (compressor, limiter, gate, expander)
- `effects`, `effects_chain` — Effects processing chain
- `eq_band` — EQ band types
- `insert_chain` — Insert effect chain
- `limiter` — Brickwall limiter
- `matrix_mixer` — Matrix mixing
- `meter_bridge`, `metering` — Meter bridge integration
- `monitor_mix` — Monitor/cue mix
- `pan_matrix` — Panning matrix
- `routing` — Signal routing
- `scene_recall`, `snapshot` — Scene/snapshot management
- `send_return` — Send/return routing
- `session` — Session management
- `sidechain` — Sidechain routing
- `vca` — VCA fader groups

**Effects categories available per channel:**
- Dynamics (compressor, limiter, gate, expander, de-esser)
- EQ (parametric, graphic, shelving, high/low pass)
- Time-based (reverb, delay, echo, chorus, flanger)
- Modulation (phaser, vibrato, tremolo, ring modulator)
- Distortion (saturation, overdrive, bit crusher, wave shaper)

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
