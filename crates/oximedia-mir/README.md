# oximedia-mir

**Status: [Stable]** | Version: 0.2.0 | Tests: extensively tested | Updated: 2026-07-14

Music Information Retrieval (MIR) system for OxiMedia, providing comprehensive music analysis including tempo, beat, key, chord, melody, structure, genre, mood, and spectral features.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

- **Tempo Detection** — BPM detection using autocorrelation and comb filtering
- **Beat Tracking** — Beat and downbeat detection with dynamic programming
- **Onset Detection** — Transient detection using spectral flux and HFC
- **Key Detection** — Musical key detection (Krumhansl-Schmuckler algorithm)
- **Chord Recognition** — Chord progression analysis using chroma features
- **Melody Extraction** — Dominant melody line extraction
- **Harmonic Analysis** — Harmonic-percussive separation
- **Structural Segmentation** — Section boundary detection (intro, verse, chorus, bridge)
- **Self-Similarity Analysis** — Pattern and repetition detection
- **Genre Classification** — Genre detection from audio features
- **Mood Detection** — Valence and arousal estimation
- **Spectral Features** — Centroid, rolloff, flux, contrast
- **Rhythm Features** — Rhythm patterns and complexity
- **Pitch Features** — Pitch class profiles and chromagrams
- **Audio Fingerprinting** — AcoustID-compatible fingerprinting
- **Chorus Detection** — Repeated section detection
- **Source Separation** — Harmonic-percussive and vocal/instrument separation
- **Cover Detection** — Identifying cover versions of songs
- **Fade Detection** — Detecting fade-in and fade-out sections
- **Vocal Detection** — Distinguishing vocal vs. instrumental sections
- **Playlist Generation** — Automatic playlist generation based on music features
- Patent-free algorithms throughout

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-mir = "0.2.0"
# Select specific features:
oximedia-mir = { version = "0.1.9", features = ["tempo", "beat", "key", "chord"] }
```

```rust
use oximedia_mir::{MirAnalyzer, MirConfig, FeatureSet};

let config = MirConfig::default();
let analyzer = MirAnalyzer::new(config);

let samples = vec![0.0_f32; 44100];
let result = analyzer.analyze(&samples, 44100.0)?;

if let Some(ref tempo) = result.tempo {
    println!("Tempo: {:.1} BPM (confidence: {:.2})", tempo.bpm, tempo.confidence);
}
if let Some(ref key) = result.key {
    println!("Key: {} (confidence: {:.2})", key.key, key.confidence);
}
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `tempo` | Tempo/BPM detection |
| `beat` | Beat tracking |
| `key` | Key detection |
| `chord` | Chord recognition |
| `melody` | Melody extraction |
| `structure` | Structural segmentation |
| `genre` | Genre classification |
| `mood` | Mood/sentiment analysis |
| `spectral` | Spectral feature extraction |
| `rhythm` | Rhythm analysis |
| `harmonic` | Harmonic analysis |
| `all` | All features (default) |

## API Overview

**Core types:**
- `MirAnalyzer` — Main analysis engine
- `MirConfig` — Analysis configuration
- `MirError` — Error type

**Modules:**
- `beat`, `beat_tracker` — Beat and tempo analysis
- `chord`, `chord_recognition` — Chord detection
- `key`, `key_detection`, `pitch_key` — Key and pitch detection
- `melody` — Melody extraction
- `rhythm`, `rhythm_pattern` — Rhythm analysis
- `spectral`, `spectral_contrast`, `spectral_features` — Spectral feature extraction
- `audio_features` — Low-level audio features
- `audio_events` — Audio event detection
- `chorus_detect` — Chorus/repeated section detection
- `cover_detect` — Cover song detection
- `fade_detect` — Fade-in/fade-out detection
- `vocal_detect` — Vocal/instrumental detection
- `fingerprint` — Audio fingerprinting (AcoustID)
- `genre`, `genre_classify` — Genre classification
- `mood`, `mood_detection` — Mood/valence/arousal estimation
- `harmonic`, `harmonic_analysis` — Harmonic analysis
- `structure`, `structure_analysis` — Song structure segmentation
- `tempo`, `tempo_map` — Tempo estimation
- `melody` — Melody extraction
- `segmentation` — General audio segmentation
- `similarity` — Music similarity comparison
- `source_separation` — Source separation
- `onset_strength` — Onset detection
- `energy_contour`, `dynamic_range` — Energy and dynamics
- `mir_feature` — Feature aggregation
- `music_summary` — Complete music summary
- `playlist`, `playlist_gen` — Playlist generation
- `tuning_detect` — Tuning/pitch deviation detection
- `loudness` — Loudness analysis
- `pitch_track` — Pitch tracking

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
