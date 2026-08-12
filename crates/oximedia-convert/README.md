# oximedia-convert

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.1](https://img.shields.io/badge/version-0.2.1-blue)

Universal media format converter for `OxiMedia`.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.1 — 2026-08-12 — extensively tested

## Features

- **Batch Conversion** — Convert multiple files with parallel processing and templates
- **Format Detection** — Auto-detect source formats and codecs
- **Conversion Profiles** — Pre-configured profiles for common use cases
- **Quality Control** — Quality comparison (PSNR/SSIM) after conversion
- **Metadata Preservation** — Preserve metadata across format conversions
- **Subtitle Conversion** — Convert between subtitle formats (SRT, `WebVTT`, ASS, etc.)
- **Audio Extraction** — Extract audio tracks in various formats
- **Video Extraction** — Extract video without audio
- **Frame Extraction** — Extract frames as images
- **Thumbnail Generation** — Generate thumbnails and sprite sheets
- **File Concatenation** — Join multiple media files
- **File Splitting** — Split files by time, size, or chapters
- **Template System** — Template-based batch conversion
- **Streaming Packaging** — ABR ladder and streaming format support
- **Image Sequences** — Export/import image sequences
- **Smart Conversion** — Content-aware optimized settings
- **Watermark Strip** — Strip embedded watermarks during conversion
- **Transcode Reporting** — Detailed conversion reports

## Usage

```rust,no_run
use oximedia_convert::{Converter, ConversionOptions, Profile, QualityMode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let converter = Converter::new();
    let options = ConversionOptions::builder()
        .profile(Profile::WebOptimized)
        .quality_mode(QualityMode::Best)
        .preserve_metadata(true)
        .build()?;

    converter.convert("input.mov", "output.mp4", options).await?;

    Ok(())
}
```

### Batch Conversion

```rust,no_run
use oximedia_convert::{BatchProcessor, ConversionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let processor = BatchProcessor::new()
        .with_max_parallel(4);

    let options = ConversionOptions::default();

    processor.process_directory(
        "input_dir",
        "output_dir",
        "*.mov",
        options,
    ).await?;

    Ok(())
}
```

### Audio Extraction

```rust,no_run
use oximedia_convert::AudioExtractor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let extractor = AudioExtractor::new()
        .as_opus()
        .with_bitrate(192_000);

    extractor.extract("video.mp4", "audio.opus").await?;

    Ok(())
}
```

### Frame Extraction

```rust,no_run
use oximedia_convert::{FrameExtractor, FrameRange};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let extractor = FrameExtractor::new()
        .as_jpeg()
        .with_quality(90);

    // Extract frame at 10 seconds
    extractor.extract_at("video.mp4", "frame.jpg", 10.0).await?;

    // Extract frames at 1 second intervals
    extractor.extract_interval("video.mp4", "frames", 1.0).await?;

    Ok(())
}
```

### Thumbnail Generation

```rust,no_run
use oximedia_convert::ThumbnailGenerator;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let generator = ThumbnailGenerator::widescreen();

    generator.generate("video.mp4", "thumbnail.jpg").await?;

    Ok(())
}
```

## API Overview

**Core types:**
- `Converter` — Main conversion engine with format detection and profile support
- `ConversionOptions` / `ConversionOptionsBuilder` — Builder-pattern conversion settings
- `ConversionReport` — Detailed report including quality metrics and duration
- `Profile` — Conversion profile enum
- `QualityMode` — Fast / Balanced / Best
- `ConversionError` — Comprehensive error type

**Modules:**
- `batch`, `batch_convert` — Batch processing with parallel execution
- `detect`, `format_detector` — Format and codec detection
- `profile`, `conv_profile` — Conversion profiles and settings
- `quality` — Quality comparison (PSNR/SSIM)
- `metadata` — Metadata extraction and preservation
- `audio` — Audio extraction
- `video` — Video extraction
- `frame` — Frame extraction
- `thumbnail`, `thumbnail_strip` — Thumbnail generation and sprite sheets
- `concat` — File concatenation
- `split` — Time/size/chapter splitting
- `template` — Template-based batch conversion
- `subtitle` — Subtitle conversion
- `streaming` — ABR ladder and HLS/DASH packaging
- `sequence` — Image sequence import/export
- `smart` — Content-aware optimized conversion
- `pipeline`, `conversion_pipeline` — Conversion pipeline execution
- `metrics` — Quality metrics
- `normalization` — Audio normalization
- `filters` — Filter chain
- `codec_mapper` — Codec mapping
- `presets` — Encoding presets
- `progress` — Progress tracking
- `watermark_strip` — Watermark removal
- `transcode_report` — Conversion reporting

## Conversion Profiles

- **WebOptimized**: MP4/H.264 for web playback
- **Streaming**: HLS/DASH for adaptive streaming
- **Archive**: Lossless MKF for preservation
- **Email**: Small file size for easy sharing
- **Mobile**: Optimized for mobile devices
- **YouTube**: Optimized for `YouTube` upload
- **Instagram**: Instagram-compliant format
- **TikTok**: TikTok-compliant format
- **Broadcast**: Broadcast-compliant MXF
- **`AudioMp3`**: Extract audio as MP3
- **`AudioFlac`**: Extract audio as FLAC (lossless)
- **`AudioAac`**: Extract audio as AAC

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
