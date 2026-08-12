# oximedia-access

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Comprehensive accessibility tools for inclusive media production in OxiMedia.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.1 — 2026-08-12 — extensively tested

## Features

### Audio Description
Generate and manage audio descriptions for blind and visually impaired users:
- Multiple AD Types: Standard, Extended, Open, and Closed
- Smart Timing: Automatic placement in dialogue gaps
- Script Management: JSON-based script format with validation
- Mixing Strategies: Replace, Mix, Duck, and Pause modes
- Quality Levels: Basic to Professional with different constraints

### Closed Captions
Advanced caption generation and styling:
- Auto-Generation: Speech-to-text based caption generation
- Smart Positioning: Automatic positioning to avoid important content
- Style Presets: Pre-configured styles for different use cases
- Synchronization: Frame-accurate timing with gap/overlap detection
- Multiple Formats: CEA-608/708, WebVTT, SRT, ASS

### Sign Language Support
Picture-in-picture sign language video overlay:
- Flexible Positioning: Corner placement or custom coordinates
- Size Presets: Small, Medium, Large, or custom
- Border Styling: Solid, Rounded, or no border
- Opacity Control: Adjustable transparency

### Transcripts
Generate and export text transcripts:
- Multiple Formats: Plain text, WebVTT, SRT, JSON
- Timestamp Precision: Millisecond-accurate timing
- Speaker Labels: Optional speaker identification
- Metadata Support: Title, language, creation date

### Translation
Multi-language subtitle translation:
- 20+ Languages: Support for major world languages
- Quality Checking: Automated translation quality metrics
- Timing Preservation: Maintains original subtitle timing
- Format Support: Works with all subtitle formats

### Text-to-Speech (TTS)
Convert text to natural speech:
- Voice Selection: Multiple voices, genders, and languages
- Prosody Control: Adjust rate, pitch, volume, emphasis
- SSML Support: Fine-grained control over speech
- Neural Voices: High-quality neural TTS support

### Speech-to-Text (STT)
Transcribe spoken content:
- Speaker Diarization: Identify different speakers
- Word Timestamps: Precise word-level timing
- Punctuation: Automatic punctuation insertion
- Domain Vocabulary: Specialized terminology support

### Visual Enhancements
Improve visual accessibility:
- Contrast Enhancement: Adjust contrast for better visibility
- Color Blindness Adaptation: Transform colors for various types
- Text Size Adjustment: Scalable text for better readability
- WCAG Compliance: Check contrast ratios against standards

### Audio Enhancements
Improve audio clarity:
- Clarity Enhancement: Boost speech intelligibility
- Noise Reduction: Remove background noise
- Loudness Normalization: EBU R128 / ATSC A/85 compliance
- Speech Focus: Enhance speech frequency range

### Speed Control
Adjust playback speed:
- Pitch Preservation: Maintain natural pitch when changing speed
- Quality Levels: Fast, Standard, and High quality modes
- Wide Range: 0.5x to 2.0x speed adjustment

### Compliance Checking
Verify accessibility standards:
- WCAG 2.1: Web Content Accessibility Guidelines (A, AA, AAA)
- Section 508: US federal accessibility standard
- EBU: European Broadcasting Union guidelines
- Detailed Reports: JSON and plain text report generation

## Architecture

The crate is organized into the following modules (72 source files, 783 public items):

- `audio_desc` — Audio description generation and mixing
- `caption` — Closed caption generation and styling
- `sign` — Sign language video overlay
- `transcript` — Transcript generation and export
- `translate` — Subtitle translation
- `tts` — Text-to-speech synthesis
- `stt` — Speech-to-text transcription
- `visual` — Visual enhancements
- `audio` — Audio enhancements
- `speed` — Playback speed control
- `compliance` — Accessibility compliance checking

## Usage Examples

### Audio Description

```rust
use oximedia_access::audio_desc::{
    AudioDescriptionGenerator, AudioDescriptionConfig,
    AudioDescriptionType, AudioDescriptionQuality,
};
use oximedia_access::audio_desc::script::AudioDescriptionScript;

// Create script
let mut script = AudioDescriptionScript::new();
script.add_entry(AudioDescriptionEntry::new(
    1000, 3000, "A sunset over mountains.".to_string()
));

// Generate audio description
let config = AudioDescriptionConfig::new(
    AudioDescriptionType::Standard,
    AudioDescriptionQuality::Standard,
);
let generator = AudioDescriptionGenerator::new(config);
let segments = generator.generate(&script)?;
```

### Closed Captions

```rust
use oximedia_access::caption::{CaptionGenerator, CaptionConfig, CaptionStyle};

let config = CaptionConfig::new("en".to_string(), CaptionType::Closed)
    .with_speaker_identification(true);

let generator = CaptionGenerator::new(config);
let captions = generator.generate_from_audio(&audio_buffer)?;

let style = CaptionStyle::from_preset(CaptionStylePreset::HighContrast)
    .with_font_size(48);
```

### Compliance Checking

```rust
use oximedia_access::compliance::{ComplianceChecker, WcagLevel};

let checker = ComplianceChecker::new();
let report = checker.check_all();

if report.is_compliant() {
    println!("Content meets accessibility standards!");
} else {
    println!("Issues found: {}", report.summary().total_issues);
    println!("{}", report.to_text());
}
```

## Integration Points

### Text-to-Speech Services
- Amazon Polly
- Google Cloud Text-to-Speech
- Microsoft Azure Speech
- IBM Watson Text to Speech
- Local engines (eSpeak, Festival, Piper)

### Speech-to-Text Services
- OpenAI Whisper
- Google Cloud Speech-to-Text
- Amazon Transcribe
- Microsoft Azure Speech
- AssemblyAI
- Local models (Vosk, DeepSpeech)

### Translation Services
- Google Translate API
- DeepL API
- Microsoft Translator
- Amazon Translate

## Standards Compliance

- **WCAG 2.1** — Level A, AA, and AAA
- **Section 508** — US federal accessibility standard
- **EBU R128** — Loudness normalization (-23 LUFS)
- **EBU-TT-D** — Subtitle format and timing

## Safety

This crate forbids unsafe code: `#![forbid(unsafe_code)]`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
