# oximedia-metering

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Professional broadcast audio metering for OxiMedia, implementing ITU-R BS.1770-4, EBU R128, and ATSC A/85 loudness standards.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **ITU-R BS.1770-4** — Algorithms to measure audio programme loudness and true-peak level
- **ITU-R BS.1771** — Loudness and true-peak indicating meter requirements
- **EBU R128** — Loudness normalisation and permitted maximum level
- **ATSC A/85** — US broadcast loudness standard
- **Momentary Loudness** — 400ms sliding window (75% overlap)
- **Short-term Loudness** — 3-second sliding window (75% overlap)
- **Integrated Loudness** — Gated program loudness (LKFS/LUFS)
- **Loudness Range (LRA)** — Percentile-based dynamic range measurement
- **True Peak Detection** — 4x oversampling with sinc interpolation
- **Per-channel Tracking** — Individual channel true peak levels
- **Multi-channel Support** — Mono through 7.1.4 Dolby Atmos
- **Compliance Checking** — EBU R128, ATSC A/85, streaming platforms (Spotify, YouTube, Apple Music)
- **VU Meters** — IEC 60268-10 standard with 300ms ballistics
- **Peak Meters** — Sample-accurate peak detection with peak hold
- **PPM Meters** — IEC standard PPM Type I/II/DIN/Nordic/BBC/SMPTE
- **Dynamic Range Metering** — DR meter for dynamic range analysis
- **K-weighting** — Frequency weighting filters (K, A, C, Z)
- **M/S Metering** — Mid-side stereo analysis
- **Phase Analysis** — Phase correlation and stereo field monitoring
- **Correlation Meter** — Stereo correlation metering
- **Spectral Analysis** — Octave band and spectrum analysis
- **Crest Factor** — Peak-to-RMS ratio measurement
- **Video Luminance/Color** — Video signal quality metrics

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-metering = "0.2.0"
```

```rust
use oximedia_metering::{LoudnessMeter, MeterConfig, Standard};

let config = MeterConfig::new(Standard::EbuR128, 48000.0, 2);
let mut meter = LoudnessMeter::new(config)?;

// Process audio samples (interleaved f32)
let audio_samples: &[f32] = &[];
meter.process_f32(audio_samples);

let metrics = meter.metrics();
println!("Integrated: {:.1} LUFS", metrics.integrated_lufs);
println!("LRA: {:.1} LU", metrics.loudness_range);
println!("True Peak: {:.1} dBTP", metrics.true_peak_dbtp);

let compliance = meter.check_compliance();
println!("Compliant: {}", compliance.is_compliant());
```

```rust
use oximedia_metering::{PeakMeter, PeakMeterType};

let mut vu_meter = PeakMeter::new(PeakMeterType::Vu, 48000.0, 2, 2.0)?;
let audio_samples: &[f64] = &[];
vu_meter.process_interleaved(audio_samples);
let peaks = vu_meter.peak_dbfs();
println!("L: {:.1} dBFS, R: {:.1} dBFS", peaks[0], peaks[1]);
```

## API Overview

**Core types:**
- `LoudnessMeter` — ITU-R BS.1770/EBU R128 loudness meter
- `MeterConfig` — Meter configuration (standard, sample rate, channels)
- `Standard` — EbuR128, AtscA85, ItuRBs1770
- `PeakMeter` — Peak/VU meter
- `PeakMeterType` — Vu, Ppm, TruePeak

**Result types:**
- `LoudnessMetrics` — Integrated, momentary, short-term LUFS, LRA, true peak
- `ComplianceReport` — Standard compliance result

**Modules:**
- `atsc` — ATSC A/85 compliance
- `ballistics` — Meter ballistics (attack/release)
- `correlation` — Stereo correlation metering
- `dr_meter`, `dynamic_range_meter` — Dynamic range (DR) metering
- `dynamics` — Dynamics analysis
- `ebu`, `ebu_r128_impl` — EBU R128 implementation
- `filters` — Frequency weighting filters
- `gating` — Loudness gating
- `k_weighting`, `k_weighted` — K-weighting filter
- `lkfs` — LKFS measurement
- `loudness_gate`, `loudness_history`, `loudness_trend` — Loudness tracking
- `m_s_meter` — Mid-side metering
- `meter_bridge`, `meter_type_config` — Meter bridge integration
- `noise_floor` — Noise floor estimation
- `octave_bands` — Octave band analysis
- `peak`, `peak_meter` — Peak detection
- `phase`, `phase_analysis`, `phase_scope` — Phase analysis
- `ppm` — PPM metering
- `range` — Loudness range
- `render`, `report` — Report rendering
- `spectral_balance`, `spectral_energy`, `spectrum`, `spectrum_bands` — Spectral analysis
- `stereo_balance` — Stereo balance metering
- `true_peak`, `truepeak` — True peak detection
- `video_color`, `video_luminance`, `video_quality` — Video signal metering
- `vu_meter` — VU meter
- `crest_factor` — Crest factor measurement

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
