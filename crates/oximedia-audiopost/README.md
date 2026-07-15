# oximedia-audiopost

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional audio post-production suite for OxiMedia: ADR, Foley, sound design, mixing, restoration, and delivery.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### ADR (Automated Dialogue Replacement)
- Session management with cue lists and timecodes
- Multiple takes per cue with ratings and selection
- Sync analysis and drift compensation
- Director notes and slate information

### Foley Recording and Editing
- Multi-track recording (up to 8 channels)
- Comprehensive foley library with categorization and tagging
- Support for multiple surface types and intensities
- Timeline integration

### Sound Design
- **Synthesizers**: Additive, Subtractive, FM, Granular, and Wavetable
- **Spatial Audio**: Stereo, 5.1, 7.1, and Atmos (7.1.4) panning
- **Effects**: Pitch shifting, time stretching, and formant preservation
- 3D positioning with distance attenuation and Doppler effect

### Mixing Console
- Professional channel strips with 4-band parametric EQ
- Gate, compressor, and limiter on each channel
- 8+ aux sends (pre/post fader)
- Master section with bus compressor and limiter
- Comprehensive metering (peak, RMS, LUFS)

### Advanced Effects
- **Dynamic**: Multiband compressor, de-esser, transient designer
- **Time-based**: Convolution and algorithmic reverb, delay, echo
- **Modulation**: Chorus, flanger, phaser, tremolo, vibrato
- **Spectral**: Vocoder, auto-tune, harmonizer, octaver

### Audio Restoration
- Spectral noise reduction with adaptive profiling
- Hiss, hum (50/60 Hz), and rumble removal
- Click, crackle, and pop removal
- Declipping and dropout repair
- Phase correction and stereo enhancement

### Stem Management
- Multi-stem creation (Dialogue, Music, Effects, Foley, Ambience)
- Independent stem processing and mixing
- DCP/IMF stem package export
- Multiple format support (WAV, FLAC, BWF)

### Loudness Management
- EBU R128, ATSC A/85, Netflix, Spotify compliance
- Integrated, short-term, and momentary loudness metering
- True peak control with look-ahead limiting
- Loudness range (LRA) measurement

### Automation
- Volume, pan, and parameter automation
- Multiple automation modes (Read, Write, Touch, Latch)
- Bezier, linear, stepped, exponential, and logarithmic curves

### Delivery and Export
- Professional deliverable specifications (Netflix, DCP, Broadcast)
- Multiple sample rates (44.1–192 kHz)
- Multiple bit depths (16, 24, 32-bit float)
- BWF and iXML metadata embedding

### Hardware Control
- MIDI controller support with customizable mappings
- OSC (Open Sound Control) integration
- Mackie Control Universal protocol
- Touch-sensitive and motorized fader support

### Session Management
- Complete project organization with tracks, clips, and regions
- Marker and region management
- Project templates (Film/TV, Podcast, Music)
- Session backup and version control

### Workflow Automation
- Batch processing with dependency management
- Job queue for multiple workflows
- Progress tracking and time estimation
- Workflow presets for common tasks

### Professional Metering
- Peak, RMS, VU, and True Peak meters
- Phase correlation and stereo width analysis
- Spectrum analyzer and goniometer
- Multi-channel metering (up to 12 channels)

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-audiopost = "0.2.0"
```

### ADR Session Example

```rust
use oximedia_audiopost::adr::{AdrSession, AdrCue, AdrTake};
use oximedia_audiopost::timecode::Timecode;

let mut session = AdrSession::new("Scene 42", 48000);

let cue = AdrCue::new(
    "Actor: 'To be or not to be'",
    Timecode::from_frames(1000, 24.0),
    Timecode::from_frames(1100, 24.0),
);
let cue_id = session.add_cue(cue);

let take = AdrTake::new(1, "/recordings/take001.wav");
session.get_cue_mut(cue_id)?.add_take(take);
```

### Mixing Console Example

```rust
use oximedia_audiopost::mixing::MixingConsole;

let mut console = MixingConsole::new(48000, 512)?;
let dialogue = console.add_channel("Dialogue")?;
let music = console.add_channel("Music")?;

console.set_channel_gain(dialogue, 6.0)?;
console.set_channel_pan(dialogue, 0.0)?; // Center
console.set_channel_gain(music, -3.0)?;
```

### Loudness Compliance Example

```rust
use oximedia_audiopost::loudness::{LoudnessMeter, LoudnessStandard};

let mut meter = LoudnessMeter::new(48000, LoudnessStandard::EbuR128)?;
meter.process(&audio_buffer);

if meter.is_compliant() {
    println!("Audio meets EBU R128 standard");
} else {
    let report = meter.get_compliance_report();
    println!("Loudness: {} LUFS (target: {})",
        report.integrated_lufs, report.target_lufs);
}
```

## Module Structure (45 source files, 1197 public items)

- `adr` — ADR session management and cue tracking
- `foley` — Foley recording and library management
- `sound_design` — Synthesizers and sound design tools
- `mixing` — Professional mixing console and channel strips
- `effects` — Advanced audio effects (dynamics, time-based, modulation, spectral)
- `restoration` — Audio restoration and repair
- `stems` — Stem management and export
- `loudness` — Loudness management and compliance
- `automation` — Parameter automation with multiple modes
- `delivery` — Professional delivery format specifications
- `midi` — MIDI hardware control integration
- `session` — Session and project management
- `workflow` — Workflow automation and batch processing
- `meters` — Professional multi-channel metering

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
