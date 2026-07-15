# oximedia-audio-analysis

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Advanced audio analysis and forensics for OxiMedia, providing comprehensive audio characterization for professional, forensic, and music applications.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Spectral Analysis** — Advanced frequency-domain analysis with multiple window functions; spectral centroid, flatness, crest factor, bandwidth, and contrast
- **Voice Analysis** — F0/formant analysis, gender detection, age estimation, emotion detection, speaker identification and verification
- **Music Analysis** — Harmonic analysis, chord progression detection, rhythmic analysis, timbral analysis, instrument identification
- **Source Separation** — Vocal/instrumental separation, drum and bass isolation via harmonic-percussive decomposition
- **Echo and Reverb Analysis** — Room acoustics measurement, RT60 estimation, early reflection pattern analysis
- **Distortion Analysis** — THD measurement, clipping detection, non-linear distortion characterization
- **Dynamic Range Analysis** — Crest factor, RMS tracking over time, loudness variation measurement
- **Transient Detection** — Attack/onset detection, ADSR envelope characterization, onset strength functions
- **Pitch Analysis** — YIN algorithm pitch tracking (patent-free), vibrato detection, F0 estimation with confidence
- **Formant Analysis** — F1–F4 tracking over time, vowel detection and classification, LPC-based formant extraction
- **Audio Forensics** — Authenticity verification, edit detection (cuts/splices), ENF analysis, compression history analysis, background noise consistency
- **Noise Analysis** — Noise profiling, SNR computation, noise floor estimation, noise type classification
- **Psychoacoustic Analysis** — Perceptual feature extraction, masking models
- **Stereo Field Analysis** — Width, phase correlation, mid/side analysis
- **Spectral Flux** — Frame-to-frame spectral change detection

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-audio-analysis = "0.2.0"
```

```rust
use oximedia_audio_analysis::{AudioAnalyzer, AnalysisConfig};

let config = AnalysisConfig::default();
let analyzer = AudioAnalyzer::new(config);

let samples = vec![0.0_f32; 44100]; // 1 second of audio
let sample_rate = 44100.0;

let result = analyzer.analyze(&samples, sample_rate)?;

println!("Spectral centroid: {:.1} Hz", result.spectral.centroid);
println!("Spectral flatness: {:.3}", result.spectral.flatness);
```

## API Overview (81 source files, 479 public items)

**Core types:**
- `AudioAnalyzer` — Main analysis entry point
- `AnalysisConfig` — Configuration for analysis parameters

**Modules:**
- `spectral` — Frequency-domain spectral analysis (FFT frame, spectral features, flux, contrast)
- `voice` — Voice characteristic analysis (characteristics, speaker identification)
- `rhythm` — Rhythmic feature extraction and tempo analysis
- `music` — Music-specific analysis (timbre, rhythm)
- `forensics` — Audio authenticity and tampering detection (compression history, noise consistency)
- `pitch` — Pitch tracking (YIN algorithm), vibrato detection
- `pitch_tracker` — Extended pitch tracking framework
- `formant` — Formant frequency analysis and vowel detection
- `noise` — Noise profiling and SNR computation
- `echo` — Echo and reverb detection and measurement
- `onset` — Transient and onset detection
- `transient` — Transient detection and envelope analysis
- `beat` — Beat tracking and rhythm analysis
- `cepstral` — Cepstral analysis for voice and music
- `harmony` — Harmonic analysis and chord detection
- `psychoacoustic` — Psychoacoustic feature extraction
- `stereo_field` — Stereo width and phase correlation
- `spectral_contrast`, `spectral_features`, `spectral_flux` — Advanced spectral metrics
- `separate` — Source separation
- `tempo_analysis` — Tempo estimation

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
