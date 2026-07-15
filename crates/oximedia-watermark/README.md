# oximedia-watermark

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional audio watermarking and steganography library for OxiMedia. Provides comprehensive audio watermarking capabilities with multiple embedding algorithms, robust detection, and psychoacoustic optimization.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

### Watermarking Algorithms

- **Spread Spectrum (DSSS)** - Robust watermarking using pseudorandom sequence spreading
- **Echo Hiding** - Single/double/triple echo watermarking with variable delays
- **Phase Coding** - DFT phase modulation in frequency domain
- **LSB Steganography** - High-capacity least significant bit embedding
- **Patchwork** - Statistical watermarking using sample pair manipulation
- **QIM** - Quantization Index Modulation for robust embedding
- **DCT Watermark** - Discrete Cosine Transform domain embedding
- **Spatial Watermark** - Spatial domain watermarking
- **Invisible Watermark** - Frequency-domain invisible embedding
- **Forensic Watermark** - Forensic-grade watermarking for content tracing
- **Fragile Watermark** - Tamper-detection watermarking
- **Visible Watermark** - Audible/visible watermark overlay
- **QR Watermark** - QR code embedded in audio for machine-readable payloads

### Key Capabilities

- **Blind Detection** - Extract watermarks without access to original audio
- **Error Correction** - Reed-Solomon coding for robustness against attacks
- **Psychoacoustic Masking** - Imperceptible watermarks using hearing model
- **Robustness Testing** - Simulate attacks (MP3, resampling, filtering, noise)
- **Quality Metrics** - Objective assessment (SNR, PSNR, ODG, LSD, WSNR)
- **Cryptographic Keys** - Secure watermarks with pseudorandom keys and key rotation
- **Batch Embedding** - Process multiple audio segments efficiently
- **Chain of Custody** - Track watermark provenance and handling history
- **Detection Map** - Spatial detection confidence mapping
- **Perceptual Hash** - Audio fingerprinting for integrity verification
- **Watermark Database** - Store and retrieve watermark records
- **Multi-algorithm Detection** - Pipeline detection across multiple algorithms

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-watermark = "0.2.0"
```

```rust
use oximedia_watermark::{WatermarkEmbedder, WatermarkDetector, WatermarkConfig, Algorithm};

// Create watermark configuration
let config = WatermarkConfig::default()
    .with_algorithm(Algorithm::SpreadSpectrum)
    .with_strength(0.1)
    .with_key(0x1234567890ABCDEF);

// Initialize embedder and detector
let embedder = WatermarkEmbedder::new(config.clone(), 44100);
let detector = WatermarkDetector::new(config);

// Embed watermark
let audio_samples: Vec<f32> = vec![0.0; 88200]; // 2 seconds at 44.1kHz
let payload = b"Copyright 2024 - All Rights Reserved";
let watermarked = embedder.embed(&audio_samples, payload)?;

// Calculate capacity
let capacity_bits = embedder.capacity(audio_samples.len());
println!("Watermark capacity: {} bits", capacity_bits);

// Detect and extract watermark
let extracted = detector.detect(&watermarked, capacity_bits)?;

// Quality assessment
use oximedia_watermark::calculate_metrics;
let metrics = calculate_metrics(&audio_samples, &watermarked);
println!("SNR: {:.2} dB", metrics.snr_db);
println!("ODG: {:.2}", metrics.odg);
```

## Algorithm Comparison

| Algorithm | Capacity | Robustness | Imperceptibility | Blind Detection |
|-----------|----------|------------|------------------|-----------------|
| Spread Spectrum | Medium | High | High | Yes |
| Echo Hiding | Low | Medium | High | Yes |
| Phase Coding | Medium | Medium | Very High | Yes |
| LSB | Very High | Low | Medium | Yes |
| Patchwork | Low | High | High | Yes |
| QIM | Medium | Very High | High | Yes |

## Quality Metrics

- **SNR** - Signal-to-Noise Ratio
- **PSNR** - Peak SNR
- **Seg-SNR** - Frame-by-frame SNR analysis
- **ODG** - Objective Difference Grade (perceptual quality, -4 to 0)
- **LSD** - Log Spectral Distance
- **WSNR** - Psychoacoustically weighted SNR

## API Overview

- `WatermarkEmbedder` — Unified embedder: embed(), capacity()
- `WatermarkDetector` — Unified detector: detect()
- `WatermarkConfig` — Configuration builder: with_algorithm(), with_strength(), with_key(), with_psychoacoustic()
- `Algorithm` — SpreadSpectrum, Echo, Phase, Lsb, Patchwork, Qim
- `AlgorithmParams` — Algorithm-specific parameter structures
- `RobustnessTest` — Attack simulation suite
- `BlindDetector` / `NonBlindDetector` / `DetectionResult` — Specialized detectors
- `QualityMetrics` / `calculate_metrics()` — Objective quality assessment
- `WatermarkError` / `WatermarkResult` — Error and result types
- Modules: `attacks`, `audio_watermark`, `batch_embed`, `bit_packing`, `chain_of_custody`, `dct_watermark`, `detection_map`, `detector`, `echo`, `error`, `forensic`, `forensic_watermark`, `fragile`, `invisible_wm`, `key_schedule`, `lsb`, `metrics`, `patchwork`, `payload`, `payload_encoder`, `perceptual_hash`, `phase`, `psychoacoustic`, `qim`, `qr_watermark`, `robust`, `robustness`, `spatial_watermark`, `spread_spectrum`, `ss_audio_wm`, `steganography`, `visible`, `visible_watermark`, `watermark_database`, `watermark_robustness`, `wm_detect`, `wm_strength`

## Ethical Use Guidelines

This library is designed for legitimate purposes only: copyright protection, broadcast monitoring, authentication, forensic tracking, and rights management. Users must comply with applicable copyright, privacy, and data protection regulations.

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
