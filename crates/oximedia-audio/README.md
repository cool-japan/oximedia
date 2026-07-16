# oximedia-audio

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Audio codec implementations and DSP tools for the OxiMedia multimedia framework.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-audio` provides encoding and decoding for royalty-free audio codecs, plus a comprehensive suite of DSP, effects, metering, spatial audio, and analysis tools.

| Codec  | Decoder | Encoder | Description |
|--------|---------|---------|-------------|
| Opus   | Yes     | Yes     | Modern, high-quality codec for speech and music |
| Vorbis | Yes     | —       | Ogg Vorbis lossy codec |
| FLAC   | Yes     | —       | Free Lossless Audio Codec |
| MP3    | Yes     | —       | MPEG-1/2 Layer III (patents expired 2017) |
| PCM    | Yes     | Yes     | Uncompressed audio |

## Features

### Codec Support

Enable specific codecs via Cargo features (all enabled by default):

```toml
[dependencies]
oximedia-audio = { version = "0.2.0", features = ["opus", "vorbis", "flac", "mp3"] }
```

### DSP

The `dsp` module provides standalone digital signal processing:
- Biquad Filters — Second-order IIR filters
- Parametric Equalizer — Multi-band EQ
- Dynamics Compressor — Full-featured compressor
- Reverb — Schroeder reverb algorithm

### Effects

The `effects` module provides modulation effects:
- Chorus — Multi-voice chorus with LFO modulation
- Flanger — Short delay with feedback and sweeping
- Phaser — All-pass filter cascade for phase shifting

### Spectrum Analysis

The `spectrum` module provides frequency-domain analysis:
- FFT Analysis — Fast Fourier Transform with window functions
- Spectrum Analyzer — Real-time frequency analysis
- Spectrogram — Time-frequency visualization
- Waveform Display — Time-domain rendering
- Feature Extraction — Spectral features and characteristics

### Audio Fingerprinting

The `fingerprint` module provides audio identification:
- Fingerprint Generation — Extract robust audio signatures
- Hash-based Matching — Fast database lookup
- Duplicate Detection — Find similar audio content

### Loudness Normalization

The `loudness` module provides broadcast-standard measurement:
- EBU R128 — European Broadcasting Union standard (-23 LUFS)
- ATSC A/85 — Advanced Television Systems Committee standard (-24 LKFS)
- ITU-R BS.1770-4 — International loudness measurement algorithm
- True Peak Detection — Prevents inter-sample clipping
- Loudness Range (LRA) — Dynamic range measurement
- K-Weighting Filter — Perceptually accurate filtering

### Audio Metering

The `meters` module provides professional metering tools:
- VU Meter — IEC 60268-10 standard with 300ms ballistics
- PPM — Peak Programme Meters (BBC, EBU, Nordic, DIN standards)
- Digital Peak Meter — Sample-accurate dBFS peak detection
- RMS Level Meter — Root mean square level measurement
- Correlation Meter — Stereo phase correlation analysis
- Goniometer — Stereo field visualization (L/R and M/S)
- LUFS Integration — Broadcast loudness metering

### Spatial Audio

The `spatial` module provides 3D spatial audio processing (binaural, ambisonics, HRTF-based panning).

### Resampling

High-quality sample rate conversion via a Pure-Rust band-limited
windowed-sinc polyphase interpolator (Blackman-Harris windowed, per-phase
DC-normalized, exact rational position tracking — no C/C++ dependencies):

```rust
use oximedia_audio::{Resampler, ResamplerQuality};

let mut resampler = Resampler::new(44100, 48000, 2, ResamplerQuality::High)?;
let output = resampler.resample(&input_frame)?;
let tail = resampler.flush()?; // drain stream tail at end of input
```

## Usage

### Opus Decoding

```rust
use oximedia_audio::OpusDecoder;

let mut decoder = OpusDecoder::new(48000, 2)?;
let frame = decoder.decode(&packet)?;
```

### Opus Encoding

```rust
use oximedia_audio::OpusEncoder;

let mut encoder = OpusEncoder::new(48000, 2, 128_000)?;
let packet = encoder.encode(&frame)?;
```

### FLAC Decoding

```rust
use oximedia_audio::FlacDecoder;

let mut decoder = FlacDecoder::new(&stream_info)?;
let frame = decoder.decode(&packet)?;
```

### Audio Frame

```rust
use oximedia_audio::{AudioFrame, ChannelLayout};

let frame = AudioFrame::new(
    48000,                  // sample rate
    1024,                   // samples per channel
    ChannelLayout::Stereo,
);
```

## Channel Layouts

| Layout       | Channels | Description                            |
|--------------|----------|----------------------------------------|
| Mono         | 1        | Single channel                         |
| Stereo       | 2        | Left, Right                            |
| Surround5_1  | 6        | FL, FR, FC, LFE, BL, BR               |
| Surround7_1  | 8        | FL, FR, FC, LFE, BL, BR, SL, SR       |

## Module Structure (115 source files, 2266 public items)

```
src/
├── lib.rs              # Crate root with re-exports
├── error.rs            # AudioError and AudioResult
├── frame.rs            # AudioFrame, AudioBuffer
├── traits.rs           # AudioDecoder, AudioEncoder traits
├── resample.rs         # Sample rate conversion (Pure-Rust windowed-sinc)
├── dsp/                # Digital signal processing
├── effects/            # Modulation effects
├── spectrum/           # Frequency-domain analysis
├── fingerprint/        # Audio fingerprinting
├── loudness/           # EBU R128 / ATSC A/85 / ITU-R BS.1770
├── meters/             # Professional audio metering
├── spatial/            # 3D spatial audio processing
├── opus/               # Opus codec (feature: opus)
├── vorbis/             # Vorbis codec (feature: vorbis)
├── flac/               # FLAC codec (feature: flac)
├── mp3/                # MP3 codec (feature: mp3)
└── pcm/                # PCM codec (feature: pcm)
```

## Codec Traits

```rust
pub trait AudioDecoder {
    fn decode(&mut self, packet: &[u8]) -> AudioResult<AudioFrame>;
    fn flush(&mut self) -> AudioResult<Vec<AudioFrame>>;
}

pub trait AudioEncoder {
    fn encode(&mut self, frame: &AudioFrame) -> AudioResult<Vec<u8>>;
    fn flush(&mut self) -> AudioResult<Vec<Vec<u8>>>;
}
```

## Policy

- No unsafe code (`#![forbid(unsafe_code)]`)
- Apache-2.0 license

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
