# oximedia-core

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.1](https://img.shields.io/badge/version-0.2.1-blue)

Core types and traits for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.1 — 2026-08-12 — extensively tested

## Overview

`oximedia-core` provides the foundational types used throughout OxiMedia:

- **Types**: Rational numbers, timestamps, pixel/sample formats, codec IDs, FourCC
- **Traits**: Decoder and demuxer interfaces
- **Error handling**: Unified error type with patent violation detection
- **Memory management**: Buffer pools for zero-copy operations
- **HDR support**: HDR metadata, transfer functions, color primaries
- **Channel layout**: Multi-channel audio layout definitions
- **Codec negotiation**: Format negotiation between components
- **Event queue / Work queue**: Internal messaging primitives

## Features

### Types

| Type | Description |
|------|-------------|
| `Rational` | Exact rational number representation (numerator/denominator) |
| `Timestamp` | Media timestamp with timebase support |
| `PixelFormat` | Video pixel format (YUV420p, NV12, P010, RGB24, etc.) |
| `SampleFormat` | Audio sample format (F32, S24, F64, planar variants, etc.) |
| `CodecId` | Codec identifier (Green List only) with `FromStr` + `canonical_name()` |
| `MediaType` | Media type classification (Video, Audio, Subtitle) |
| `FourCC` | Registry-oriented four-character code (`crate::fourcc`) |
| `FourCc` | Typed `repr(transparent)` FourCC value with 30+ named constants (`types::fourcc`) |
| `ColorPrimaries` | ITU-T H.273 primary chromaticities (BT.709, BT.2020, P3-D65, …) |
| `TransferCharacteristics` | Electro-optical transfer function (SDR, PQ/SMPTE 2084, HLG, …) |
| `MatrixCoefficients` | YCbCr derivation matrix (BT.601, BT.2020 NCL/CL, ICtCp, …) |
| `ColorRange` | Quantisation range: `Limited` (studio swing) or `Full` (PC range) |

### Error Handling

| Error Type | Description |
|------------|-------------|
| `IoError` | I/O operation failures |
| `FormatError` | Container format issues |
| `CodecError` | Codec-specific errors |
| `PatentViolation` | Attempted use of patent-encumbered codec |

### Memory Management

| Type | Description |
|------|-------------|
| `BufferPool` | Zero-copy buffer allocation and reuse |

## Usage

```rust
use oximedia_core::types::{Rational, Timestamp, PixelFormat, CodecId};
use oximedia_core::error::OxiResult;

fn example() -> OxiResult<()> {
    // Create a timestamp at 1 second with millisecond precision
    let ts = Timestamp::new(1000, Rational::new(1, 1000));
    assert!((ts.to_seconds() - 1.0).abs() < f64::EPSILON);

    // Check codec properties
    let codec = CodecId::Av1;
    assert!(codec.is_video());

    // Check pixel format properties
    let format = PixelFormat::Yuv420p;
    assert!(format.is_planar());
    assert_eq!(format.plane_count(), 3);

    Ok(())
}
```

## FourCC Constants

`oximedia-core` ships two FourCC types with different purposes:

- **`crate::fourcc::FourCC`** — Registry-oriented type (`is_video()`, `is_audio()`, `parse()`).
- **`crate::types::fourcc::FourCc`** — Lightweight `repr(transparent)` value type for
  compile-time constants and `std::str::FromStr` integration.

### `FourCc` usage

```rust
use oximedia_core::types::fourcc::{FourCc, AVC1, AV01, MP4A, OPUS, FLAC_, FTYP, MOOV, APV1, JXL_};

// Named constants — zero runtime cost
assert_eq!(AV01.as_bytes(), b"av01");
assert_eq!(APV1.as_bytes(), b"apv1");  // APV intra-frame professional codec
assert_eq!(JXL_.as_bytes(), b"jxl ");  // JPEG-XL ISOBMFF brand (trailing space)
assert_eq!(FTYP.as_bytes(), b"ftyp");
assert_eq!(MOOV.as_bytes(), b"moov");

// Parse from a string (case-exact, must be exactly 4 bytes)
let code: FourCc = "av01".parse().expect("4-byte string");
assert_eq!(code, AV01);

// User-defined constant
const MY_BOX: FourCc = FourCc::new(*b"mdhd");
```

Named constant groups:

| Group | Constants |
|-------|-----------|
| Video codecs | `AVC1`, `HVC1`, `HEV1`, `AV01`, `VP08`, `VP09`, `MJPG_AVI`, `MJPG_MP4`, `APV1`, `JXL_`, `FFV1` |
| Audio codecs | `MP4A`, `OPUS`, `FLAC_`, `VRBIS` |
| ISOBMFF boxes | `FTYP`, `MOOV`, `MDAT`, `MOOF`, `TRAK`, `MDIA`, `MINF`, `STBL`, `STSD`, `SIDX` |
| RIFF / AVI | `RIFF`, `LIST`, `AVI_`, `AVIX`, `IDX1`, `MOVI` |

## PixelFormat — Hardware Interop Variants

In addition to the standard planar and packed formats, `PixelFormat` provides semi-planar
formats commonly output by hardware video decoders:

| Variant | Bits | Layout | Typical source |
|---------|------|--------|----------------|
| `Nv12` | 8 | Y plane + interleaved UV | Most hardware decoders |
| `Nv21` | 8 | Y plane + interleaved VU | Android camera HAL |
| `P010` | 10 | Y plane + interleaved UV (16-bit words) | 10-bit HDR hardware decode/encode |
| `P016` | 16 | Y plane + interleaved UV (16-bit words) | Full 16-bit HW precision |

## SampleFormat — Extended Precision

`SampleFormat` covers the complete range from 8-bit unsigned to 64-bit double-precision:

| Variant | Bits | Type | Notes |
|---------|------|------|-------|
| `U8` | 8 | unsigned int | |
| `S16` / `S16p` | 16 | signed int | interleaved / planar |
| `S24` / `S24p` | 24 | signed int | 3 bytes/sample; WAV/professional audio |
| `S32` / `S32p` | 32 | signed int | |
| `F32` / `F32p` | 32 | float | default; ~144 dB dynamic range |
| `F64` / `F64p` | 64 | double | highest precision |

## CodecId — String Parsing and New Variants

`CodecId` implements `std::str::FromStr` (case-insensitive) and `Display`. Parsing accepts
common aliases (`jxl` → `JpegXl`, `mjpg` → `Mjpeg`, `exr` → `OpenExr`, etc.).

```rust
use oximedia_core::types::CodecId;

// Image codecs added in recent versions
let webp = CodecId::WebP;
let gif  = CodecId::Gif;
let jxl  = CodecId::JpegXl;

// Round-trip through string
assert_eq!("webp".parse::<CodecId>().unwrap(), webp);
assert_eq!("gif".parse::<CodecId>().unwrap(), gif);
assert_eq!("jxl".parse::<CodecId>().unwrap(), jxl);
assert_eq!(jxl.canonical_name(), "jpegxl");

// APV — royalty-free professional intra-frame codec (ISO/IEC 23009-13)
let apv: CodecId = "apv".parse().unwrap();
assert!(apv.is_video());
```

## Wave 4 Additions (0.1.4)

### Color Metadata Enums

Four compact enums in `color_metadata` encode ITU-T H.273 / ISO 23001-8 colour description
parameters, enabling zero-overhead codec integration:

| Enum | Const fn | Description |
|------|----------|-------------|
| `ColorPrimaries` | `to_u8()` / `from_u8()` | Primary chromaticities: Bt709, Bt2020, DciP3, P3D65, … |
| `TransferCharacteristics` | `to_u8()` / `from_u8()` + `is_hdr()` | Transfer function: Smpte2084 (PQ), Hlg, Srgb, … |
| `MatrixCoefficients` | `to_u8()` / `from_u8()` | Derivation matrix: Bt601, Bt2020Ncl, ICtCp, … |
| `ColorRange` | — | `Limited` (studio swing) or `Full` (PC range) |

`TransferCharacteristics::is_hdr()` returns `true` for `Smpte2084`, `Hlg`, `Bt2020_10`,
and `Bt2020_12`.

```rust
use oximedia_core::color_metadata::{ColorPrimaries, TransferCharacteristics,
                                    MatrixCoefficients, ColorRange};

// HDR10 descriptor
let primaries = ColorPrimaries::Bt2020;
let transfer  = TransferCharacteristics::Smpte2084;
let matrix    = MatrixCoefficients::Bt2020Ncl;
assert!(transfer.is_hdr());
assert_eq!(ColorPrimaries::from_u8(primaries.to_u8()), primaries);
```

### `Timestamp` Arithmetic Methods

Three new methods for wall-clock-based timestamp manipulation (saturating, timebase-aware):

| Method | Description |
|--------|-------------|
| `duration_add(Duration)` | Advance PTS by a `std::time::Duration`; saturates at `i64::MAX` |
| `duration_sub(Duration)` | Retreat PTS; clamps to 0 on underflow |
| `scale_by(num, den)` | Multiply PTS by `num/den`; returns `self` unchanged when `den == 0` |

### Immersive Audio Channel Layouts

Three new variants in `channel_layout::ChannelLayoutKind`:

| Variant | Channels | Bed | Height | Use case |
|---------|----------|-----|--------|----------|
| `Surround714` | 11 | 7.1 | 3 | Dolby Atmos 7.1.4 |
| `Surround916` | 16 | 9.1 | 6 | Auro-3D / Atmos 9.1.6 |
| `DolbyAtmosBed9_1_6` | 16 | 9.1 | 6 | Dolby Atmos canonical bed order |

All three report `has_height_channels() == true`.

## Module Structure

```
src/
├── lib.rs              # Crate root with re-exports
├── error.rs            # OxiError and OxiResult
├── types/
│   ├── mod.rs
│   ├── rational.rs     # Rational number type
│   ├── timestamp.rs    # Timestamp with timebase
│   ├── pixel_format.rs # Video pixel formats (incl. NV12/NV21/P010/P016)
│   ├── sample_format.rs # Audio sample formats (incl. S24/F64)
│   ├── codec_id.rs     # Codec identifiers (FromStr, canonical_name)
│   └── fourcc.rs       # FourCc value type and 30+ named constants
├── traits/
│   ├── mod.rs
│   ├── decoder.rs      # VideoDecoder trait
│   └── demuxer.rs      # Demuxer trait
├── alloc/
│   └── buffer_pool.rs  # Zero-copy buffer pool
├── buffer_pool.rs      # Buffer pool (crate root level)
├── channel_layout.rs   # Multi-channel audio layouts
├── codec_info.rs       # Codec information
├── codec_negotiation.rs # Format negotiation
├── convert.rs          # Type conversions
├── error_context.rs    # Contextual error wrapping
├── event_queue.rs      # Internal event queue
├── fourcc.rs           # FourCC code support
├── frame_info.rs       # Frame metadata
├── hdr.rs              # HDR metadata and transfer functions
├── media_time.rs       # Media time utilities
├── memory.rs           # Memory management
├── pixel_format.rs     # Pixel format (crate root level)
├── rational.rs         # Rational arithmetic
├── resource_handle.rs  # Resource lifecycle management
├── sample_format.rs    # Sample format (crate root level)
├── sync.rs             # Synchronization primitives
├── type_registry.rs    # Runtime type registry
├── version.rs          # Version information
├── work_queue.rs       # Work queue primitive
└── wasm.rs             # WASM bindings (wasm32 target only)
```

## Green List (Supported Codecs)

| Category | Codecs |
|----------|--------|
| Video | AV1, VP9, VP8, Theora, MJPEG, APV, FFV1, H.263* |
| Image | JPEG-XL, WebP, GIF, PNG, TIFF, OpenEXR, DNG |
| Audio | Opus, Vorbis, FLAC, PCM, MP3* |
| Subtitle | WebVTT, ASS/SSA, SRT |

\* H.263 and MP3 patents expired; included for compatibility.

Attempting to use patent-encumbered codecs (H.264, H.265, AAC, etc.) will result in a `PatentViolation` error.

## Feature Flags

| Feature | Description |
|---------|-------------|
| `wasm` | WASM bindings via wasm-bindgen |

## Policy

- No warnings (clippy pedantic)
- Apache 2.0 license

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
