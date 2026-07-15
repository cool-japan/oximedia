# oximedia-caption-gen

Advanced caption and subtitle generation — speech alignment, line breaking, WCAG compliance, and speaker diarization for OxiMedia

[![Crates.io](https://img.shields.io/crates/v/oximedia-caption-gen.svg)](https://crates.io/crates/oximedia-caption-gen)
[![Documentation](https://docs.rs/oximedia-caption-gen/badge.svg)](https://docs.rs/oximedia-caption-gen)
[![License](https://img.shields.io/crates/l/oximedia-caption-gen.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- Frame-accurate speech-to-caption alignment with word-level and segment-level timestamps
- Automatic segment merging (short segments) and splitting (long segments at sentence/word boundaries)
- Caption block construction with configurable max lines and characters per line
- Greedy and optimal (Knuth-Plass DP) line-breaking algorithms minimizing raggedness
- Reading speed (CPS) computation, validation, and duration adjustment
- WCAG 2.1 compliance checking: caption coverage (1.2.2), live latency (1.2.4), reading speed, minimum duration, gap detection
- Speaker diarization: turn merging, per-speaker statistics, dominant speaker detection, crosstalk detection
- Speaker-to-caption block assignment based on temporal overlap
- Voice activity ratio computation with interval union
- Pure Rust with zero C/Fortran dependencies

## Quick Start

```toml
[dependencies]
oximedia-caption-gen = "0.2.0"
```

### Speech Alignment and Caption Blocks

```rust
use oximedia_caption_gen::alignment::{
    TranscriptSegment, WordTimestamp, align_to_frames, build_caption_blocks,
};

let mut segment = TranscriptSegment {
    text: "Hello world".to_string(),
    start_ms: 0,
    end_ms: 2000,
    speaker_id: None,
    words: vec![
        WordTimestamp { word: "Hello".to_string(), start_ms: 0, end_ms: 1000, confidence: 0.95 },
        WordTimestamp { word: "world".to_string(), start_ms: 1000, end_ms: 2000, confidence: 0.92 },
    ],
};

// Map words to frame numbers at 25 fps
let frames = align_to_frames(&segment, 25.0).unwrap();
assert_eq!(frames[0].0, 0);  // "Hello" at frame 0
assert_eq!(frames[1].0, 25); // "world" at frame 25

// Build caption blocks with 2 lines, 42 chars per line
let blocks = build_caption_blocks(&[segment], 2, 42);
assert_eq!(blocks[0].id, 1);
```

### Line Breaking

```rust
use oximedia_caption_gen::line_breaking::{greedy_break, optimal_break, LineBalance};

let text = "This is a sample caption text for demonstration";

// Greedy: break at last space before max width
let greedy = greedy_break(text, 20);

// Optimal (Knuth-Plass DP): minimize squared slack for balanced lines
let optimal = optimal_break(text, 20);

// Optimal produces better-balanced lines
let opt_balance = LineBalance::balance_factor(&optimal);
let greed_balance = LineBalance::balance_factor(&greedy);
assert!(opt_balance <= greed_balance + 0.01);
```

### WCAG 2.1 Compliance

```rust
use oximedia_caption_gen::wcag::{run_all_checks, compliance_score, WcagLevel};
use oximedia_caption_gen::alignment::{CaptionBlock, CaptionPosition};

let blocks = vec![
    CaptionBlock {
        id: 1, start_ms: 0, end_ms: 2000,
        lines: vec!["Hello world".to_string()],
        speaker_id: None, position: CaptionPosition::Bottom,
    },
    CaptionBlock {
        id: 2, start_ms: 2000, end_ms: 4000,
        lines: vec!["How are you".to_string()],
        speaker_id: None, position: CaptionPosition::Bottom,
    },
];

let violations = run_all_checks(&blocks, 4000, WcagLevel::AA);
let score = compliance_score(&violations); // 100.0 if no violations
```

### Speaker Diarization

```rust
use oximedia_caption_gen::diarization::{
    DiarizationResult, Speaker, SpeakerTurn,
    speaker_stats, dominant_speaker, assign_speakers_to_blocks,
    CrosstalkDetector, voice_activity_ratio,
};

let mut result = DiarizationResult::new();
result.speakers.insert(1, Speaker {
    id: 1, name: Some("Alice".to_string()), gender: None, language: None,
});
result.turns = vec![
    SpeakerTurn { speaker_id: 1, start_ms: 0, end_ms: 5000 },
    SpeakerTurn { speaker_id: 2, start_ms: 5000, end_ms: 10000 },
];

let stats = speaker_stats(&result);
let dominant = dominant_speaker(&result); // Some(1) or Some(2)
let var = voice_activity_ratio(&result, 12000); // fraction of content with speech
let overlaps = CrosstalkDetector::find_overlapping_turns(&result);
```

## Modules

### `alignment`

Core types `WordTimestamp` (word text, start/end ms, ASR confidence) and `TranscriptSegment` (text, timing, optional speaker, word list). `align_to_frames` maps segments to `(frame_number, subtitle_line)` pairs at a given FPS, supporting both word-level and segment-level alignment. `merge_short_segments` absorbs segments shorter than a threshold into adjacent segments. `split_long_segments` breaks oversized segments at sentence then word boundaries with proportional timestamp redistribution. `build_caption_blocks` wraps segments into `CaptionBlock` values with greedy line wrapping and configurable max lines. `CaptionPosition` supports Bottom, Top, and Custom(x%, y%) placement.

### `line_breaking`

`greedy_break` wraps text at the last space before `max_width`. `optimal_break` uses a Knuth-Plass-inspired dynamic programming algorithm minimizing `sum((max_width - line_width)^2)` for more balanced output. `LineBreakConfig` holds broadcast-standard defaults (42 chars/line, 17 CPS, 2 lines, 80ms gap). `compute_cps` and `reading_speed_ok` validate reading speed in characters per second. `adjust_duration_for_reading` computes the minimum display time for a given CPS limit. `LineBalance::balance_factor` scores line balance from 0.0 (perfect) to 1.0 (maximally unbalanced). `rebalance_lines` attempts to improve balance by re-running the optimal algorithm.

### `wcag`

WCAG 2.1 accessibility compliance checks organized by success criteria:

- **`check_caption_coverage`** (1.2.2, Level A) -- detects gaps exceeding 2 seconds between caption blocks
- **`check_live_latency`** (1.2.4, Level AA) -- validates live caption latency is under 3 seconds
- **`check_sign_language`** (1.2.6, Level AAA) -- placeholder (not machine-checkable)
- **`check_cps`** -- validates reading speed against BBC/Netflix 17 CPS guideline
- **`check_min_duration`** -- enforces minimum 1-second display time per block
- **`check_gap_duration`** -- finds all inter-block gaps exceeding a threshold

`run_all_checks` executes all checks appropriate for a target `WcagLevel`. `compliance_score` computes a 0-100 score with configurable penalties per severity level. `WcagViolation` provides rule ID, message, severity level, and optional timestamp.

### `diarization`

`Speaker` metadata (id, name, gender, language) and `SpeakerTurn` (speaker id, start/end ms) with overlap detection. `DiarizationResult` aggregates speakers and turns. `merge_consecutive_turns` joins same-speaker turns separated by less than 500ms. `speaker_stats` computes per-speaker total time, turn count, and average turn duration. `dominant_speaker` identifies the speaker with the most airtime. `assign_speakers_to_blocks` maps speakers to caption blocks by maximum temporal overlap. `CrosstalkDetector::find_overlapping_turns` detects simultaneous speech. `voice_activity_ratio` computes the fraction of content with active speech using interval union. `format_speaker_label` generates display names.

## Architecture

The caption generation pipeline flows from raw transcript segments through alignment, merging/splitting, line breaking, and finally caption block construction. WCAG checks can be run as a post-processing validation step on the generated blocks. Diarization is an orthogonal pipeline that can be integrated at the block level via `assign_speakers_to_blocks`. All functions are stateless and operate on slices, making them composable in streaming or batch workflows. The only external dependency is `thiserror`.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)
