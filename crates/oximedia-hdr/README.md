# oximedia-hdr

[![Crates.io](https://img.shields.io/crates/v/oximedia-hdr.svg)](https://crates.io/crates/oximedia-hdr)
[![Documentation](https://docs.rs/oximedia-hdr/badge.svg)](https://docs.rs/oximedia-hdr)
[![License](https://img.shields.io/crates/l/oximedia-hdr.svg)](LICENSE)
![Tests: 506](https://img.shields.io/badge/tests-506-brightgreen)
![Updated: 2026-08-12](https://img.shields.io/badge/updated-2026--08--12-blue)

HDR (High Dynamic Range) video processing for [OxiMedia](https://github.com/cool-japan/oximedia) -- the Sovereign Media Framework.

Pure Rust implementation of industry-standard HDR transfer functions, tone-mapping operators, gamut conversion, and metadata handling for HDR10, HDR10+, HLG, and Dolby Vision workflows.

## Features

- **Transfer Functions** -- SMPTE ST 2084 (PQ) OETF/EOTF, ARIB STD-B67 (HLG) OETF/EOTF, and configurable SDR gamma with a unified `TransferFunction` enum
- **Tone Mapping** -- 7 operators (Reinhard, Reinhard Extended, Hable, ACES, Hable Full, Clamp, Reinhard2) with per-pixel RGB mapping, exposure/saturation controls, and whole-frame processing
- **Gamut Conversion** -- Rec.709, Rec.2020, P3-D65, P3-DCI, and ACES AP0 gamuts with 3x3 RGB-to-RGB matrices computed via CIE XYZ and Bradford chromatic adaptation; process-wide `OnceLock<RwLock<>>` cache deduplicates matrix computation across frames (two-phase read → write-on-miss locking via `parking_lot`)
- **HDR10/HDR10+ Metadata** -- SMPTE ST 2086 mastering display colour volume, CTA-861.3 content light level, and ST 2094-40 dynamic metadata with SEI encode/decode
- **HLG Advanced** -- Complete BT.2100 HLG system model (OETF, EOTF, OOTF with system gamma), HLG-to-SDR conversion with BT.2020-to-BT.709 matrix, and HLG-to-PQ cross-format conversion
- **Colour Volume** -- MDCV and CLL SEI payload parsing/encoding in HEVC big-endian format, luminance weight computation from primaries
- **Dolby Vision** -- Profile detection (P4/P5/P7/P8/P9), RPU header data, base-layer signal compatibility, and cross-version backward-compatibility checks

## Quick Start

```rust
use oximedia_hdr::{
    TransferFunction, ToneMapper, ToneMappingConfig, ToneMappingOperator,
    GamutConversionMatrix, ColorGamut, HlgSystem, HlgSdrConvert,
    MasteringDisplayColorVolume, ContentLightLevel,
    parse_hdr10_sei, encode_hdr10_sei, parse_cll_sei, encode_cll_sei,
    DolbyVisionProfile, detect_profile, DvMetadata, BlSignalCompatibility,
};

// PQ transfer function round-trip
let pq = TransferFunction::Pq;
let encoded = pq.from_linear(0.01).unwrap();  // 100 nits / 10000
let decoded = pq.to_linear(encoded).unwrap();

// Tone-map HDR10 to SDR with the Hable filmic curve
let config = ToneMappingConfig::hdr10_to_sdr();
let mapper = ToneMapper::new(config);
let (r, g, b) = mapper.map_pixel(0.8, 0.5, 0.3);

// Convert Rec.2020 to Rec.709
let mat = GamutConversionMatrix::rec2020_to_rec709().unwrap();
let (r709, g709, b709) = mat.convert(0.5, 0.4, 0.3);

// HLG to SDR conversion (BT.2100 system model)
let converter = HlgSdrConvert::default();
let (sr, sg, sb) = converter.hdr_to_sdr(0.5, 0.4, 0.3).unwrap();

// Parse / encode SMPTE ST 2086 mastering display SEI
let vol = MasteringDisplayColorVolume::rec2020_reference();
let sei_bytes = encode_hdr10_sei(&vol);
let parsed = parse_hdr10_sei(&sei_bytes).unwrap();

// Dolby Vision profile detection
let profile = DolbyVisionProfile::from_number(8).unwrap();
assert_eq!(profile.description(), "HDR10 base + DV RPU metadata (Profile 8)");
```

## Modules

| Module | Description |
|--------|-------------|
| `transfer_function` | PQ (ST 2084), HLG (STD-B67), SDR gamma OETF/EOTF with `TransferFunction` enum |
| `tone_mapping` | 7 tone-mapping operators, `ToneMappingConfig`, per-pixel and per-frame mapping |
| `gamut` | `ColorGamut` enum (Rec.709/2020, P3, ACES), `GamutConversionMatrix` via Bradford CAT |
| `dynamic_metadata` | HDR10+ per-frame dynamic metadata (ST 2094-40) with SEI encode/decode |
| `metadata` | `HdrFormat` enum, `HdrMasteringMetadata` (ST 2086), `ContentLightLevel` (CTA-861) |
| `hlg_advanced` | `HlgSystem` (OETF/EOTF/OOTF per BT.2100), `HlgSdrConvert`, `HlgHdr10Convert` |
| `color_volume` | `MasteringDisplayColorVolume`, `ContentLightLevel`, HEVC SEI parse/encode helpers |
| `dolby_vision_profile` | `DolbyVisionProfile` (P4/5/7/8/9), `DvMetadata`, `detect_profile`, RPU data |

## Supported Colour Gamuts

| Gamut | White Point | Description |
|-------|-------------|-------------|
| Rec.709 | D65 | HD broadcast / sRGB |
| Rec.2020 | D65 | UHD / HDR television |
| P3-D65 | D65 | HDR cinema displays |
| P3-DCI | DCI | Digital cinema projectors |
| ACES AP0 | ~D60 | Academy Color Encoding System |

## Tone-Mapping Operators

| Operator | Formula | Use Case |
|----------|---------|----------|
| Reinhard | `L / (1 + L)` | General purpose, soft roll-off |
| ReinhardExtended | `L * (1 + L/Lw^2) / (1 + L)` | White-point aware |
| Hable | Uncharted 2 filmic curve | Film-like contrast |
| ACES | Narkowicz 2015 fitted approximation | Cinema-grade |
| HableFull | Hable with 2x exposure bias | Higher contrast filmic |
| Clamp | Hard clip at 1.0 | Reference / testing |
| Reinhard2 | Luminance-preserving (Eq. 4) | Academic reference |

## Error Handling

All fallible operations return `oximedia_hdr::Result<T>`, backed by the `HdrError` enum:

- `InvalidLuminance` -- out-of-range or negative luminance values
- `UnsupportedTransferFunction` -- unknown transfer function identifier
- `GamutConversionError` -- singular matrix or invalid pixel buffer length
- `MetadataParseError` -- SEI payload too short or invalid format tag
- `ToneMappingError` -- pixel buffer length not divisible by 3

## Dolby Vision Profiles

| Profile | Base Layer | Enhancement Layer | Use Case |
|---------|------------|-------------------|----------|
| P4 | HDR10 (PQ/BT.2020) | Dolby Vision RPU | UHD Blu-ray |
| P5 | Dolby Vision native | None | Single-layer streaming |
| P7 | SDR/HLG | Full DV enhancement | Cinema distribution |
| P8 | HDR10 (PQ/BT.2020) | None (RPU metadata only) | Streaming (most common) |
| P9 | SDR (BT.709) | None (RPU metadata only) | Broadcast backward-compat |

## Standards Compliance

- SMPTE ST 2084 (PQ) -- Perceptual Quantizer EOTF
- SMPTE ST 2086 -- Mastering Display Colour Volume
- SMPTE ST 2094-40 -- HDR10+ Dynamic Metadata
- ITU-R BT.2020 -- UHD colour gamut
- ITU-R BT.2100 -- HLG/PQ HDR television
- ARIB STD-B67 -- Hybrid Log-Gamma
- CTA-861.3 -- Content Light Level

## License

Copyright (c) COOLJAPAN OU (Team Kitasan). All rights reserved.

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) project.
