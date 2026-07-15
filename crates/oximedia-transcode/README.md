# oximedia-transcode

![Status: Stable](https://img.shields.io/badge/status-stable-green)

High-level transcoding pipeline for OxiMedia with professional features and industry-standard presets.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Simple One-Liner API** - Quick transcoding with sensible defaults
- **Industry-Standard Presets** - YouTube, Vimeo, broadcast, streaming, and more
- **Multi-Pass Encoding** - 2-pass and 3-pass encoding for optimal quality
- **ABR Ladder Generation** - Adaptive bitrate encoding for HLS/DASH
- **Parallel Encoding** - Encode multiple outputs simultaneously
- **Progress Tracking** - Real-time progress with ETA estimation
- **Audio Normalization** - Automatic loudness normalization (EBU R128/ATSC A/85)
- **Quality Control** - CRF, CBR, VBR, and constrained VBR modes
- **Hardware Acceleration** - Auto-detection and use of GPU encoders
- **Filter Chains** - Video and audio filter pipelines
- **Codec Optimization** - Codec-specific configurations (H.264, VP9, AV1, Opus)
- **Job Management** - Queue, schedule, and prioritize transcode jobs
- **Subtitle Burn-in** - Burn subtitles into video or embed as separate stream
- **Scene Cut Detection** - Detect scene boundaries for optimal segmentation
- **Rate-Distortion Analysis** - Optimal encoding parameter selection
- **Segment Encoding** - Encode media in independent segments
- **AB Comparison** - Compare two encoded outputs for quality assessment
- **Watermark Overlay** - Embed graphics or text during transcoding
- **Concat Transcode** - Join multiple media sources during transcode

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-transcode = "0.2.0"
```

```rust
use oximedia_transcode::{Transcoder, presets};

// Simple transcode to YouTube 1080p
Transcoder::new()
    .input("input.mp4")
    .output("output.mp4")
    .preset(presets::youtube::youtube_1080p())
    .transcode()
    .await?;
```

```rust
use oximedia_transcode::{TranscodePipeline, MultiPassMode, QualityMode};

// Create HLS ABR ladder with multiple qualities
TranscodePipeline::builder()
    .input("source.mp4")
    .abr_ladder(presets::streaming::hls_ladder())
    .audio_normalize(true)
    .quality(QualityMode::High)
    .parallel_encode(true)
    .multipass(MultiPassMode::TwoPass)
    .progress(|p| {
        println!("Progress: {}% - ETA: {:?}", p.percent, p.eta);
    })
    .execute()
    .await?;
```

## Supported Platforms

### Streaming
- YouTube (1080p60, 4K, VP9/H.264)
- Vimeo (Professional quality)
- Twitch (Live streaming)
- Social Media (Instagram, TikTok, Twitter)

### Broadcast
- ProRes Proxy (HD/4K)
- DNxHD Proxy (Avid)
- EBU R128 Compliant
- ATSC A/85 Compliant

### Streaming Protocols
- HLS (HTTP Live Streaming)
- DASH (MPEG-DASH)
- CMAF (Common Media Application Format)

### Archive
- Lossless/Near-Lossless (FFV1)
- VP9/AV1 Archival

## API Overview

- `Transcoder` — Simple fluent API: input(), output(), preset(), video_codec(), audio_codec(), multi_pass(), quality(), transcode()
- `TranscodeConfig` — Configuration: input, output, codecs, bitrates, resolution, frame rate, subtitle/chapter modes
- `TranscodeOutput` — Result: output path, file size, duration, bitrates, encoding time, speed factor
- `TranscodePipeline` — Complex pipeline builder with ABR, normalization, parallel encoding
- `AbrLadder` / `AbrRung` / `AbrStrategy` — Adaptive bitrate ladder construction
- `MultiPassMode` / `MultiPassEncoder` — Two-pass and three-pass encoding
- `QualityMode` / `QualityPreset` / `RateControlMode` — Quality and rate control
- `HwAccelType` / `HwEncoder` / `HwAccelConfig` — Hardware acceleration management
- `AudioNormalizer` / `LoudnessStandard` / `LoudnessTarget` — Audio loudness normalization
- `ParallelEncoder` / `ParallelConfig` — Concurrent multi-output encoding
- `JobQueue` / `TranscodeJob` / `JobPriority` / `TranscodeStatus` — Job management
- `ProgressInfo` / `ProgressCallback` / `ProgressTracker` — Real-time progress
- `PresetConfig` — Preset container for video/audio codec, bitrate, resolution, frame rate
- `CodecConfig` / `H264Config` / `Vp9Config` / `Av1Config` / `OpusConfig` — Codec-specific settings
- `AudioFilter` / `VideoFilter` / `FilterNode` — Filter chain nodes
- `SubtitleMode` / `ChapterMode` — Subtitle and chapter handling
- `TranscodeError` / `Result` — Error and result types
- Modules: `ab_compare`, `abr_ladder`, `adaptive_bitrate`, `audio_channel_map`, `audio_transcode`, `bitrate_control`, `bitrate_estimator`, `burn_subs`, `codec_mapping`, `codec_profile`, `concat_transcode`, `crop_scale`, `crf_optimizer`, `encoding_log`, `examples`, `frame_stats`, `output_verify`, `presets`, `rate_distortion`, `resolution_select`, `scene_cut`, `segment_encoder`, `segment_transcoder`, `stage_graph`, `thumbnail`, `transcode_metrics`, `transcode_session`, `two_pass`, `utils`, `validation`, `watermark_overlay`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
