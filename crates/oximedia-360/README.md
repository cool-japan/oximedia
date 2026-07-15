# oximedia-360

360° VR video processing — equirectangular/cubemap projections, stereo 3D, spatial metadata for OxiMedia

[![Crates.io](https://img.shields.io/crates/v/oximedia-360.svg)](https://crates.io/crates/oximedia-360)
[![Documentation](https://docs.rs/oximedia-360/badge.svg)](https://docs.rs/oximedia-360)
[![License](https://img.shields.io/crates/l/oximedia-360.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- Equirectangular, spherical, and cubemap coordinate transforms with bilinear sampling
- Full equirectangular-to-cubemap and cubemap-to-equirectangular image conversion
- Side-by-side and top-bottom stereo frame splitting/merging
- Depth-based stereo synthesis with parallax mapping
- Four fisheye projection models: equidistant, equisolid, orthographic, stereographic
- Dual-fisheye stitching with linear-alpha blend in the overlap zone
- Google Spatial Media v2 XMP serialization/parsing and ISOBMFF `sv3d`/`st3d` box encoding
- Stereo quality metrics (parallax error via SAD)
- Zero C/Fortran dependencies

## Quick Start

```toml
[dependencies]
oximedia-360 = "0.2.0"
```

### Equirectangular to Cubemap

```rust
use oximedia_360::projection::{equirect_to_cube, CubeFace};

let src_rgb = vec![128u8; 256 * 128 * 3]; // 256x128 equirect image
let faces = equirect_to_cube(&src_rgb, 256, 128, 64).unwrap();
assert_eq!(faces.len(), 6); // one 64x64 face per cube side
```

### Stereo Frame Splitting

```rust
use oximedia_360::stereo::{split_stereo_frame, StereoLayout};

let frame = vec![0u8; 1920 * 1080 * 3]; // side-by-side stereo
let (left, right) = split_stereo_frame(&frame, 1920, 1080, StereoLayout::LeftRight).unwrap();
assert_eq!(left.len(), 960 * 1080 * 3);
```

### Fisheye to Equirectangular

```rust
use oximedia_360::fisheye::{fisheye_to_equirect, FisheyeParams};

let fisheye_img = vec![100u8; 512 * 512 * 3];
let params = FisheyeParams::equidistant(180.0);
let equirect = fisheye_to_equirect(&fisheye_img, 512, 512, &params, 1024, 512).unwrap();
assert_eq!(equirect.len(), 1024 * 512 * 3);
```

### Spatial Media XMP

```rust
use oximedia_360::spatial_metadata::SpatialMediaV2;

let meta = SpatialMediaV2::equirectangular_mono();
let xmp = meta.to_xmp();
assert!(xmp.contains("equirectangular"));

// Round-trip parse
let parsed = SpatialMediaV2::parse_xmp(&xmp).unwrap();
assert_eq!(parsed.projection, meta.projection);
```

## Modules

### `projection`

Coordinate types (`SphericalCoord`, `UvCoord`, `CubeFace`, `CubeFaceCoord`) and conversion functions between equirectangular UV, spherical coordinates, and cube-face coordinates. Includes `bilinear_sample_u8` for edge-clamped bilinear sampling from 8-bit image buffers. The `equirect_to_cube` and `cube_to_equirect` functions perform full image-level conversions between equirectangular panoramas and six cube-map faces.

### `stereo`

Stereoscopic 3D support with `StereoLayout` (TopBottom, LeftRight, Alternating, Mono), frame splitting and merging, `StereoCalibration` for computing pixel disparities from physical camera parameters, `DepthMap` for normalized depth storage, and `stereo_from_depth` for synthesizing stereo pairs via horizontal pixel shift. `StereoQuality` provides parallax error measurement via average SAD.

### `fisheye`

Four fisheye lens projection models (`FisheyeModel`: Equidistant, Equisolid, Orthographic, Stereographic) with forward and inverse projection functions. `FisheyeParams` describes lens FOV, optical centre, and radius. Conversion functions `fisheye_to_equirect` and `equirect_to_fisheye` handle full image reprojection. `DualFisheyeStitcher` stitches front and back fisheye images into a single equirectangular frame with configurable blend width. `detect_horizon` computes the horizon elevation angle for a given lens configuration.

### `spatial_metadata`

Google Spatial Media v2 metadata with `SpatialMediaV2` supporting XMP serialization (`to_xmp`) and parsing (`parse_xmp`). `ProjectionType` covers equirectangular, cubemap, fisheye, and mesh projections. ISOBMFF box types `Sv3dBox` (spherical video) and `StereoVideoBox` (`st3d`) provide binary serialization for embedding 360° metadata in MP4 containers.

## Architecture

All angles are in radians. UV coordinates are normalized to 0..1. Image buffers use packed RGB (3 bytes per pixel, row-major) format throughout. The crate performs no heap allocation for coordinate-level operations; image-level functions allocate output buffers. The only external dependency is `thiserror`.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)
