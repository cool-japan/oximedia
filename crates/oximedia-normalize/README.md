# oximedia-normalize

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Professional broadcast loudness normalization for OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Overview

`oximedia-normalize` provides comprehensive loudness normalization compliant with all major broadcast and streaming standards, including EBU R128, ATSC A/85, and streaming platform requirements.

## Features

### Broadcast Standards Support

- **EBU R128** - European Broadcasting Union (-23 LUFS ±1 LU, -1 dBTP max)
- **ATSC A/85** - US broadcast standard (-24 LKFS ±2 dB, -2 dBTP max)
- **Streaming Platforms** - Spotify, YouTube, Apple Music, Tidal, Netflix, etc.
- **ReplayGain** - Album and track gain (reference 89 dB SPL)

### Processing Modes

- **Two-pass Normalization** - Analyze first, then apply precise gain
- **One-pass Normalization** - Real-time with lookahead buffer
- **Linear Gain** - Simple gain adjustment to target loudness
- **Dynamic Normalization** - DRC for consistent loudness across content
- **True Peak Limiting** - Brick-wall limiter preventing clipping

### Advanced Features

- **Multi-pass Processing** - Iterative refinement for high-precision normalization
- **Batch Processing** - Process entire directories of audio files
- **Real-time Processing** - Low-latency normalization for live applications
- **Metadata Writing** - ReplayGain, R128, iTunes Sound Check tags
- **Compliance Checking** - Verify against all broadcast standards
- **Auto Gain Control (AGC)** - Automatic gain control for live content
- **DC Offset Removal** - Remove DC bias from audio
- **Dialogue Normalization** - Dialogue-specific normalization
- **Spectral Balance** - Frequency-aware normalization
- **Stem Loudness** - Multi-stem loudness management
- **Stereo Width** - Stereo width processing
- **Sidechain Processing** - Sidechain-based normalization
- **Voice Activity Detection** - VAD-aware normalization
- **Phase Correction** - Phase correction during normalization

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-normalize = "0.2.0"
```

## Quick Start

### Two-pass Normalization

```rust
use oximedia_normalize::{Normalizer, NormalizerConfig};
use oximedia_metering::Standard;

// Configure for EBU R128 normalization
let config = NormalizerConfig::new(Standard::EbuR128, 48000.0, 2);
let mut normalizer = Normalizer::new(config)?;

// Pass 1: Analyze
normalizer.analyze_f32(audio_samples);
let analysis = normalizer.get_analysis();
println!("Current: {:.1} LUFS, Target: {:.1} LUFS",
         analysis.integrated_lufs,
         analysis.target_lufs);

// Pass 2: Normalize
let mut output = vec![0.0f32; audio_samples.len()];
normalizer.process_f32(audio_samples, &mut output)?;
```

### Real-time Normalization

```rust
use oximedia_normalize::{RealtimeNormalizer, RealtimeConfig};
use oximedia_metering::Standard;

let config = RealtimeConfig::new(Standard::Spotify, 48000.0, 2);
let mut normalizer = RealtimeNormalizer::new(config)?;

// Process audio chunks
loop {
    let chunk = get_next_audio_chunk();
    let mut output = vec![0.0f32; chunk.len()];
    normalizer.process_chunk(&chunk, &mut output)?;
    send_to_output(&output);
}
```

### Batch Processing

```rust
use oximedia_normalize::{BatchProcessor, BatchConfig};
use oximedia_metering::Standard;

let config = BatchConfig::new(Standard::Spotify);
let processor = BatchProcessor::new(config);

// Process entire directory
let results = processor.process_directory(
    Path::new("input/"),
    Path::new("output/")
)?;

// Generate report
let report = BatchProcessor::generate_report(&results);
println!("{}", report.format());
```

## Workflow Guide: Two-pass vs. One-pass vs. Batch

| | `Normalizer` (two-pass) | `RealtimeNormalizer` (one-pass) | `BatchProcessor` / `batch_normalizer::BatchNormalizer` |
|---|---|---|---|
| Latency | Whole-file (offline) | `lookahead_ms` only (tens of ms) | Whole-file, per item |
| Gain accuracy | Exact — computed from the complete gated integrated loudness before any output is written | Approximate — a smoothed running estimate (`RealtimeConfig::smoothing_time_s`) that always trails true program loudness by design | Exact per item, plus optional cross-item scheduling |
| Needs full input up front | Yes | No — streams sample-by-sample | Yes, but decoupled per file |
| Typical use | Archive/VOD/podcast mastering & delivery | Live streaming, broadcast contribution, capture pipelines | Music libraries, podcast back-catalogs, album releases |

- **Two-pass (`Normalizer`)** — use whenever the full signal is available (files, in-memory clips). `analyze_f32`/`analyze_f64` measure the whole program once; `process_f32`/`process_f64` then apply one precise, constant gain. This is the only mode that reliably lands the output loudness inside a standard's compliance tolerance, because the gain comes from a fully gated measurement rather than a running estimate.
- **One-pass (`RealtimeNormalizer`)** — use for live/streaming sources where you cannot buffer the whole program. `process_chunk` measures only a lookahead window (exposed as `latency_samples()`), rides a smoothed gain, and can run the result through `TruePeakLimiter` so inter-sample peaks never clip even though the loudness estimate itself is still approximate. Use `RealtimeConfig::low_latency` to trade measurement stability for lower delay.
- **Batch (`BatchProcessor` / `batch_normalizer::BatchNormalizer`)** — use for collections of files. `batch_normalizer::BatchNormalizer` is the working two-pass batch engine: `measure` every item, `schedule_gains` in `GainMode::Independent` (each file hits the target on its own — right for standalone tracks/episodes) or `GainMode::Album` (every file shares one gain derived from the loudest item, preserving relative loudness across an album), then `apply_to_item`. `batch::BatchProcessor` exposes the same `BatchConfig`/`BatchResult` shape and its `process_file`/`process_directory` now decode/normalize/encode real WAV files end-to-end (analyze → gain → optional limiter/DRC → write); `write_metadata` is still inert (no tags are embedded yet) and non-WAV formats are not yet supported — use `batch_normalizer::BatchNormalizer` directly for those cases or for in-memory sample buffers.

## Relationship with `oximedia-metering`

`oximedia-normalize` does not re-implement loudness measurement — every gain decision here is derived from analysis performed by `oximedia-metering`:

- `analyzer::LoudnessAnalyzer` (used by `Normalizer` and by `batch::BatchProcessor`) wraps `oximedia_metering::LoudnessMeter`, the standard-aware ITU-R BS.1770-4 meter that measures integrated loudness, LRA, true peak, and momentary/short-term loudness for any `oximedia_metering::Standard`. `analyzer::AnalysisResult` layers normalization-specific fields on top of the raw `oximedia_metering::LoudnessMetrics`: `recommended_gain_db`, `safe_gain_db`/`max_safe_gain_db` (largest gain that keeps true peak under the standard's ceiling), and `is_compliant`/`compliance` (via `oximedia_metering::ComplianceResult`).
- `realtime::RealtimeNormalizer` drives its own `oximedia_metering::LoudnessMeter` internally to produce the running estimate its gain smoothing responds to.
- `replaygain::ReplayGainCalculator` uses the same meter, measured against the fixed -18 LUFS ReplayGain reference (`replaygain::REPLAYGAIN_REFERENCE_LUFS`) instead of a broadcast/streaming `Standard` target.
- `metering_bridge` provides a lighter, standard-agnostic vocabulary (`LufsTarget`, `MeteringWindow`, `LoudnessMeasurement`) for bridging externally-supplied measurements into gain plans without depending on `oximedia_metering`'s `Standard`/`LoudnessMeter` types directly.

In short: `oximedia-metering` answers "how loud is this, and is it compliant?"; `oximedia-normalize` answers "what gain (and what limiting/DRC) gets it there safely?" and applies it.

## Architecture

### Core Modules

- **`analyzer`** - Two-pass loudness analysis using ITU-R BS.1770-4
- **`processor`** - Normalization processing with gain, limiting, and DRC
- **`limiter`, `limiter_chain`** - True peak limiter with lookahead buffering
- **`drc`** - Broadcast-quality dynamic range compressor
- **`targets`, `loudness_target`** - Target loudness standards and presets
- **`replaygain`** - ReplayGain calculation and tagging
- **`metadata`** - Loudness metadata writing (ID3v2, Vorbis, APE, MP4)
- **`batch`** - Batch file processing
- **`realtime`** - Real-time normalization with low latency
- **`multipass`** - Multi-pass processing controller
- **`agc`, `auto_gain`** - Automatic gain control
- **`broadcast_standard`** - Broadcast standard definitions
- **`compliance_checker`** - Standards compliance verification
- **`dc_offset`** - DC offset removal
- **`dialogue_norm`** - Dialogue normalization
- **`dynamic_range`** - Dynamic range processing
- **`ebu_r128`** - EBU R128 implementation
- **`fade_normalization`** - Fade-aware normalization
- **`format_loudness`** - Format-specific loudness settings
- **`gain_schedule`** - Gain scheduling
- **`loudness_history`** - Loudness history tracking
- **`metering_bridge`** - Integration with oximedia-metering
- **`multi_channel_loud`** - Multi-channel loudness
- **`noise_profile`** - Noise profile analysis
- **`normalize_report`** - Normalization reporting
- **`peak_limit`** - Peak limiting
- **`phase_correction`** - Phase correction
- **`sidechain`** - Sidechain processing
- **`spectral_balance`** - Spectral balance normalization
- **`stem_loudness`** - Stem-level loudness management
- **`stereo_width`** - Stereo width processing
- **`target_loudness`** - Target loudness configuration
- **`true_peak_limiter`** - True peak brick-wall limiter
- **`voice_activity`** - Voice activity detection

### Processing Pipeline

```
Input Audio
    ↓
K-weighting Filter (ITU-R BS.1770-4)
    ↓
Loudness Analysis (Gating, Integration)
    ↓
Gain Calculation
    ↓
Gain Application
    ↓
Dynamic Range Compression (optional)
    ↓
True Peak Limiting (optional)
    ↓
Output Audio
```

## Standard Selection Guide

`oximedia_metering::Standard` is the source of truth for target loudness and max true
peak (`Standard::target_lufs()` / `Standard::max_true_peak_dbtp()`); the table below adds
recommended *processing* settings for this crate on top of that.

| Target | Standard | Target LUFS | Max True Peak | Recommended config |
|---|---|---|---|---|
| EBU R128 (EU broadcast/OTT) | `Standard::EbuR128` | -23.0 | -1.0 dBTP | `NormalizerConfig::broadcast` — limiter + DRC on, 10 ms lookahead, 15 dB max gain, metadata on (R128 tags) |
| ATSC A/85 (US broadcast) | `Standard::AtscA85` | -24.0 | -2.0 dBTP | `NormalizerConfig::broadcast` — same rationale as EBU R128; the wider ±2 dB tolerance permits slightly more aggressive DRC |
| Spotify | `Standard::Spotify` | -14.0 | -1.0 dBTP | `NormalizerConfig::new` (two-pass, limiter on, DRC off) — Spotify does not reward over-compression and turns down content louder than target itself |
| YouTube | `Standard::YouTube` | -14.0 | -1.0 dBTP | Same as Spotify: limiter-only two-pass; YouTube also turns down rather than boosts |
| Apple Music | `Standard::AppleMusic` | -16.0 | -1.0 dBTP | `NormalizerConfig::new` with limiter on; enable `write_metadata` to emit the iTunes Sound Check tag so Apple's playback gain matches your measurement |
| Netflix | `Standard::Netflix` | -27.0 (dialogue-gated drama) | -2.0 dBTP | Use `cinema_loudness`/`dialogue_gate` dialogue-gated measurement rather than a plain program-loudness two-pass — Netflix's delivery spec measures dialogue loudness, not full-program loudness |
| Amazon Prime Video | `Standard::AmazonPrime` | -24.0 | -2.0 dBTP | `NormalizerConfig::broadcast`-style settings (closer to ATSC A/85 than to the -14 LUFS music platforms) |
| ReplayGain (personal libraries, offline players) | not an `oximedia_metering::Standard` variant | -18.0 (fixed reference) | player-dependent | `replaygain::ReplayGainCalculator` via batch processing in `GainMode::Album` for whole albums (preserves track-to-track dynamics) or `Independent` for shuffled/single-track playback |

Rules of thumb:

- **Broadcast standards (EBU R128, ATSC A/85, Amazon Prime)** expect tightly controlled
  loudness *and* peaks: always enable the true-peak limiter, prefer
  `NormalizerConfig::broadcast`, and write metadata so downstream QC tools agree with your
  measurement.
- **Streaming music platforms (Spotify, YouTube, Apple Music, Tidal, Amazon Music HD)**
  normalize on playback and penalize over-compressed masters — keep DRC off and let the
  limiter run only as a safety net against inter-sample peaks.
- **Dialogue-centric long-form content (film/TV/Netflix)** should be measured with
  `dialogue_gate`/`cinema_loudness` rather than plain full-program integration, since
  minutes of ambient-only content can otherwise skew a gated measurement away from
  perceived dialogue loudness.
- **ReplayGain** fits best when there is no single delivery platform to target — it
  stores a gain *offset* rather than a baked-in target, so playback software can apply it
  consistently across an eclectic personal collection.

### Broadcast

| Standard | Target LUFS | Max Peak | Tolerance |
|----------|-------------|----------|-----------|
| EBU R128 | -23.0 | -1.0 dBTP | ±1.0 LU |
| ATSC A/85 | -24.0 | -2.0 dBTP | ±2.0 dB |
| BBC iPlayer | -23.0 | -1.0 dBTP | ±1.0 LU |

### Streaming Platforms

| Platform | Target LUFS | Max Peak |
|----------|-------------|----------|
| Spotify | -14.0 | -1.0 dBTP |
| YouTube | -14.0 | -1.0 dBTP |
| Apple Music | -16.0 | -1.0 dBTP |
| Tidal | -14.0 | -1.0 dBTP |
| Netflix (Drama) | -27.0 | -2.0 dBTP |
| Amazon Prime | -24.0 | -2.0 dBTP |

## Technical Details

### Loudness Measurement

- **ITU-R BS.1770-4** compliant K-weighting filter
- **Absolute gate** at -70 LKFS
- **Relative gate** at -10 LU below ungated loudness
- **True peak detection** via 4x oversampling with sinc interpolation

### True Peak Limiting

- Lookahead buffer (configurable, default 5-10ms)
- 4x oversampling for accurate peak detection
- Attack/release envelope shaping
- Zero artifacts brick-wall limiting

### Dynamic Range Compression

- Configurable threshold, ratio, attack, release
- Soft knee for smooth compression
- Automatic makeup gain
- Broadcast-style envelope following

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
