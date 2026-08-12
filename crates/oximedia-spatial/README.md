# oximedia-spatial

[![Crates.io](https://img.shields.io/crates/v/oximedia-spatial.svg)](https://crates.io/crates/oximedia-spatial)
[![docs.rs](https://img.shields.io/docsrs/oximedia-spatial)](https://docs.rs/oximedia-spatial)
[![License](https://img.shields.io/crates/l/oximedia-spatial.svg)](LICENSE)

**Spatial audio processing for [OxiMedia](https://github.com/cool-japan/oximedia)** -- Higher-Order Ambisonics, HRTF binaural rendering, room acoustics, VBAP panning, head tracking, Wave Field Synthesis, and ADM object-based audio.

Pure Rust. No C/Fortran dependencies. Patent-free.

## Features

- **Ambisonics** -- HOA encoding/decoding up to 3rd order (16 channels), ACN channel ordering, N3D/SN3D/FuMa normalisation, stereo and 5.1 decoding
- **Binaural rendering** -- Synthetic HRTF database (120 measurements), ITD/ILD modelling, linear convolution, static and moving-source rendering with crossfade
- **Room simulation** -- Image-source method with 6-wall first-order reflections, exponentially decaying late reverberation tail, preset rooms (small room, concert hall, studio)
- **VBAP** -- 2D panning for speaker rings and 3D panning for full-sphere layouts, energy-normalised gains, automatic pair/triplet triangulation
- **Head tracking** -- Quaternion orientation with SLERP, complementary filter fusing gyroscope and accelerometer, low-pass and Kalman post-filter smoothing, binaural angle compensation
- **Wave Field Synthesis** -- Linear and circular array definitions, point-source and plane-wave virtual sources, 2.5D driving function computation, per-speaker delay/gain calculation
- **Object audio** -- ADM metadata parsing, AudioObject with position/gain/spread/divergence, ObjectRenderer with speaker layouts from stereo to 9.1.6, Gaussian panning with energy normalisation

## Quick start

```rust
use oximedia_spatial::ambisonics::{AmbisonicsEncoder, AmbisonicsOrder, SoundSource};

let encoder = AmbisonicsEncoder::new(AmbisonicsOrder::First, 48_000);
let source = SoundSource::new(45.0, 0.0);
let mono = vec![0.5_f32; 256];
let channels = encoder.encode_mono(&mono, &source);
assert_eq!(channels.len(), 4); // W, Y, Z, X
```

## Module reference

### `ambisonics`

`AmbisonicsEncoder` encodes mono or stereo audio into B-format channel sets at 1st, 2nd, or 3rd order. `AmbisonicsDecoder` decodes B-format to stereo or 5.1 surround via mode-matched spherical harmonic evaluation at speaker positions. Source positions are specified in azimuth/elevation degrees with distance and gain.

### `binaural`

`HrtfDatabase::synthetic()` generates 120 HRTF measurements covering the sphere (24 azimuths x 5 elevations). `BinauralRenderer` convolves mono audio with the nearest HRTF pair, supporting both static positions and moving-source paths with per-segment crossfading.

### `room_simulation`

`RoomSimulator` implements the image-source method for a shoebox room. `RoomConfig` provides presets (`small_room`, `concert_hall`, `recording_studio`) with configurable absorption and RT60. `generate_rir()` produces a room impulse response combining direct sound, early reflections, and a late reverberation tail. `apply_reverb()` convolves dry audio with the RIR.

### `vbap`

`VbapPanner` handles 2D VBAP for horizontal speaker rings -- finds the bracketing speaker pair, inverts the 2x2 direction matrix, and returns energy-normalised gains. `VbapPanner3d` extends this to full-sphere layouts using speaker triplets with 3x3 matrix inversion.

### `head_tracking`

`HeadTracker` maintains a `Quaternion` orientation estimate from gyroscope integration with optional accelerometer correction via a complementary filter. Post-filter smoothing options include `LowPass` (exponential moving average) and `Kalman` (per-axis angle+rate filter). `to_binaural_angles()` transforms world-space source directions into head-relative coordinates.

### `wave_field`

`WfsArray` defines linear or circular loudspeaker arrays with per-speaker positions and normals. `WfsRenderer` computes 2.5D driving functions (complex-valued) and per-speaker delay/gain pairs for `VirtualSource` objects (point sources or plane waves).

### `object_audio`

`AudioObject` carries ADM-style Cartesian position, gain, spatial spread (width/height/depth), and divergence. `ObjectRenderer` maps objects to speaker-bed channels (stereo through 9.1.6) using Gaussian panning with energy normalisation. `parse_adm_object()` extracts object metadata from minimal ADM XML fragments.

## Speaker layouts

| Layout | Channels | Description |
|--------|----------|-------------|
| Stereo | 2 | L, R |
| 5.1 | 6 | L, R, C, LFE, Ls, Rs |
| 7.1 | 8 | 5.1 + Lss, Rss |
| 7.1.4 | 12 | 7.1 + Ltf, Rtf, Ltr, Rtr |
| 9.1.6 | 16 | 7.1 + Lts, Rts + 6 height channels |

## Error handling

All fallible operations return `Result<T, SpatialError>`. Variants:

- `InvalidConfig` -- invalid parameters (e.g. too few speakers for VBAP)
- `ParseError` -- malformed ADM XML attributes
- `ComputationError` -- degenerate matrix inversion or similar

## Coordinate convention

- **Azimuth**: 0 = front, 90 = left, 180 = back, 270 = right (degrees, CCW)
- **Elevation**: 0 = horizontal, +90 = above, -90 = below (degrees)
- **ADM Cartesian**: x in [-1,1] (right = +1), y in [-1,1] (front = +1), z in [-1,1] (up = +1)

## License

Copyright (c) COOLJAPAN OU (Team Kitasan). All rights reserved.

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) Sovereign Media Framework.

Version: 0.2.1 — 2026-08-12 — extensively tested
