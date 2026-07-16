# oximedia-bench

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Comprehensive codec benchmarking suite for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Multi-codec support** — Benchmark AV1, VP9, VP8, and Theora codecs
- **Quality metrics** — PSNR, SSIM, and optional VMAF calculations (feature: `vmaf`)
- **Performance metrics** — Encoding/decoding speed, memory usage, CPU utilization
- **Statistical analysis** — Mean, median, percentiles, standard deviation, advanced stats (via `statistical` crate)
- **Parallel execution** — Multi-threaded benchmark execution via rayon
- **Report generation** — Export results in JSON, CSV, HTML, and Markdown formats
- **Result caching** — Incremental benchmarking with result caching
- **Preset configurations** — Quick, standard, comprehensive, quality-focused, and speed-focused presets

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-bench = "0.2.0"
# or without VMAF:
oximedia-bench = { version = "0.2.0", default-features = false }
```

```rust
use oximedia_bench::{BenchmarkConfig, BenchmarkSuite, CodecConfig};
use oximedia_core::types::CodecId;

// Create a benchmark configuration
let config = BenchmarkConfig::builder()
    .add_codec(CodecConfig::new(CodecId::Av1))
    .add_codec(CodecConfig::new(CodecId::Vp9))
    .parallel_jobs(4)
    .build()?;

// Run the benchmark
let suite = BenchmarkSuite::new(config);
let results = suite.run_all()?;

// Export results
results.export_json("results.json")?;
results.export_csv("results.csv")?;
results.export_html("results.html")?;
```

## API Overview (28 source files, 616 public items)

**Core types:**
- `BenchmarkSuite` — Main benchmark orchestrator
- `BenchmarkConfig` — Configuration builder for codec and benchmark parameters
- `CodecConfig` — Per-codec benchmark settings
- `BenchmarkResults` — Aggregated benchmark results with export methods

**Modules:**
- `lib.rs` — Main benchmarking framework and configuration
- `metrics` — Quality metrics (PSNR, SSIM, VMAF)
- `sequences` — Test sequence management and generation
- `runner` — Benchmark execution engine
- `comparison` — Cross-codec comparison tools
- `report` — Report generation (JSON/CSV/HTML/Markdown)
- `stats` — Statistical analysis utilities (mean, median, percentiles, standard deviation)
- `examples` — Usage examples and integration patterns

## Feature Flags

| Feature | Description |
|---------|-------------|
| `vmaf` | Enable VMAF quality metric calculation (default: enabled) |

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
