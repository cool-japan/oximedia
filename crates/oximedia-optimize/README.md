# oximedia-optimize

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Codec optimization and tuning suite for OxiMedia. Provides advanced optimization techniques for video encoders, including rate-distortion optimization, psychovisual tuning, and adaptive quantization.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Rate-Distortion Optimization (RDO)** - Advanced mode decision based on rate-distortion curves with RDOQ
- **Psychovisual Optimization** - Perceptual quality tuning using visual masking models and contrast sensitivity
- **Motion Search Tuning** - Advanced algorithms (TZSearch, EPZS, UMH) for motion estimation
- **Intra Prediction Optimization** - RDO-based directional mode selection
- **Transform Optimization** - Adaptive transform selection (DCT/ADST) and quantization
- **Loop Filter Tuning** - Deblocking and Sample Adaptive Offset (SAO) optimization
- **Partition Selection** - Complexity-based block size decision trees
- **Reference Frame Management** - Optimal DPB management and reference selection
- **Adaptive Quantization** - Variance and psychovisual-based AQ modes
- **Entropy Coding Optimization** - Context modeling for CABAC/CAVLC
- **Lookahead Analysis** - Temporal optimization with configurable lookahead frames
- **Two-pass Encoding** - Two-pass bitrate allocation
- **Bitrate Control** - Advanced bitrate controller and optimizer
- **Quality Ladder** - Adaptive quality ladder generation
- **Scene Detection** - Scene-based encoding optimization
- **Frame Budget** - Frame-level bit budget allocation
- **GOP Optimization** - GOP structure optimization
- **Cache Optimization** - Encoder cache strategy and prefetching
- **Complexity Analysis** - Content complexity analysis
- **Perceptual Optimization** - Perceptual quality-aware encoding
- **Transcode Optimization** - Transcoding pipeline optimization
- **Benchmark** — Encoding benchmark and profiling

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-optimize = "0.2.0"
```

```rust
use oximedia_optimize::{OptimizerConfig, OptimizationLevel, Optimizer};

let config = OptimizerConfig {
    level: OptimizationLevel::Slow,
    enable_psychovisual: true,
    enable_aq: true,
    lookahead_frames: 40,
    ..Default::default()
};

let optimizer = Optimizer::new(config)?;
```

## API Overview

**Core types:**
- `Optimizer` — Main optimization engine coordinating all sub-systems
- `OptimizerConfig` — Configuration with optimization level, psychovisual, AQ, and lookahead settings
- `OptimizationLevel` — Presets: Fast, Medium, Slow, Placebo
- `ContentType` — Content hints: Animation, Film, Screen, Generic

**Modules:**
- `adaptive_ladder` — Adaptive bitrate ladder
- `aq` — Adaptive quantization
- `benchmark` — Encoding benchmark tools
- `bitrate_controller`, `bitrate_optimizer` — Bitrate control
- `cache_opt`, `cache_optimizer`, `cache_strategy` — Cache optimization
- `complexity_analysis` — Content complexity analysis
- `crf_sweep` — CRF sweep for quality targeting
- `decision` — Encoding decision engine
- `encode_preset`, `encode_stats` — Preset and statistics
- `entropy` — Entropy coding optimization
- `examples` — Usage examples
- `filter` — Filter optimization
- `frame_budget` — Frame-level bit budget
- `gop_optimizer` — GOP structure optimization
- `intra` — Intra prediction optimization
- `lookahead` — Lookahead analysis
- `media_optimize` — Media-level optimization
- `motion` — Motion estimation optimization
- `parallel_strategy` — Parallel encoding strategies
- `partition` — Block partition optimization
- `perceptual_optimization` — Perceptual quality tuning
- `prefetch` — Prefetch optimization
- `presets` — Optimization presets
- `psycho` — Psychovisual analysis
- `quality_ladder`, `quality_metric` — Quality ladder and metrics
- `quantizer_curve` — Quantizer curve modeling
- `rdo` — Rate-distortion optimization engine
- `reference` — Reference frame management
- `scene_encode` — Scene-based encoding
- `strategies` — Optimization strategies
- `transcode_optimizer` — Transcoding optimization
- `transform` — Transform optimization
- `two_pass` — Two-pass encoding
- `utils` — Utility functions

**Key types:**
- `RdoEngine` / `RdoResult` — Rate-distortion optimization
- `PsychoAnalyzer` / `VisualMasking` — Psychovisual analysis
- `MotionOptimizer` / `MotionVector` — Motion estimation
- `AqEngine` / `AqMode` — Adaptive quantization
- `LookaheadAnalyzer` / `GopStructure` — Lookahead-based GOP optimization
- `BenchmarkRunner` / `Profiler` — Encoding benchmark tools

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
