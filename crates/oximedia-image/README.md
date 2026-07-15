# oximedia-image

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 1198](https://img.shields.io/badge/tests-1198-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

Professional image sequence I/O for OxiMedia, supporting DPX, OpenEXR, and TIFF with full color depth and cinema-grade processing.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **DPX** — SMPTE 268M-2003 v2.0 Digital Picture Exchange format
- **OpenEXR** — High dynamic range format with deep image support
- **TIFF / BigTIFF** — Tagged Image File Format with LZW/Deflate/ZIP compression
- **Raw Camera Decode** — Camera RAW format decoding
- Full color depth: 8, 10, 12, 16-bit, float (f32), half-float (f16)
- Linear and logarithmic color spaces
- Metadata preservation (camera, display window, EXIF, XMP)
- Sequence pattern matching (printf-style `%04d` and `#` hash notation)
- Parallel I/O with rayon
- Zero-copy operations where possible
- **HDR Merge** — Multi-exposure HDR merge
- **Inpainting** — Image inpainting for region repair
- **Stitching** — Image stitching/panorama
- **Lens Correction** — Lens distortion correction
- **Image Pyramid** — Gaussian/Laplacian pyramid processing
- **Tone Curve** — Tone curve application (log, sRGB, gamma)
- **Color Adjustment** — Brightness, contrast, saturation, curves
- **Color Balance** — Shadows/midtones/highlights color balance
- **Color Science** — Colorimetric conversions and white balance
- **Convolution** — Arbitrary kernel convolution
- **Edge Detection** — Sobel, Canny, Laplacian
- **Morphology** — Erosion, dilation, open/close
- **Histogram** — Histogram equalization, CLAHE, operations
- **Thumbnail Cache** — Thumbnail generation and persistent cache
- **ICC Embedding** — ICC profile embedding
- **Blend Modes** — Photoshop-compatible blend modes
- **Noise Generation** — Procedural noise (Perlin, Gaussian, film grain)
- **Mosaic** — Bayer mosaic and demosaicing
- **Pattern Generation** — Test pattern and chart generation
- **Format Detection** — Auto-detect image format

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-image = "0.2.0"
```

```rust
use oximedia_image::{ImageSequence, SequencePattern};

fn example() -> Result<(), Box<dyn std::error::Error>> {
    // Load a DPX sequence
    let pattern = SequencePattern::parse("render.%04d.dpx")?;
    let sequence = ImageSequence::from_pattern(pattern, 1..=100)?;

    // Read frame 50
    let frame = sequence.read_frame(50)?;
    println!("Frame 50: {}x{}", frame.width, frame.height);
    Ok(())
}
```

## API Overview

**Core types:**
- `ImageSequence` — Image sequence with frame access
- `SequencePattern` — Sequence filename pattern (printf-style or hash)
- `ImageData`, `ImageFrame` — Image data container
- `PixelType` — U8, U10, U12, U16, F16, F32
- `ImageError`, `ImageResult` — Error types

**Format modules:**
- `dpx` — DPX reader/writer
- `exr` — OpenEXR reader/writer
- `tiff` — TIFF/BigTIFF reader/writer
- `raw`, `raw_decode` — Camera RAW decode
- `format_detect` — Auto-detect image format
- `sequence` — Image sequence management

**Color processing:**
- `color_adjust` — Brightness, contrast, saturation
- `color_balance` — Shadows/midtones/highlights balance
- `color_science` — Colorimetric conversions
- `tone_curve` — Tone curve application
- `blend_mode` — Blend mode operations
- `icc_embed` — ICC profile embedding

**Image processing:**
- `convolution` — Arbitrary kernel convolution
- `filter`, `filters` — Common image filters
- `edge_detect` — Edge detection algorithms
- `morphology` — Morphological operations
- `pyramid` — Image pyramid (Gaussian/Laplacian)
- `histogram_ops` — Histogram operations
- `transform` — Geometric transforms

**Advanced operations:**
- `hdr_merge` — Multi-exposure HDR merge
- `inpaint` — Image inpainting
- `stitch` — Image stitching
- `lens_correct` — Lens distortion correction
- `depth_map` — Depth map operations
- `noise_gen` — Procedural noise generation
- `mosaic` — Bayer mosaic/demosaicing
- `pixel_pipeline` — Pixel processing pipeline
- `dither_engine` — Dithering for bit depth reduction

**Metadata:**
- `exif_parser` — EXIF metadata parsing
- `metadata_xmp` — XMP metadata
- `thumbnail_cache` — Thumbnail generation and caching

**Other:**
- `channel_ops` — Channel extraction/combination
- `crop_region` — Crop region handling
- `pattern` — Test pattern generation

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
