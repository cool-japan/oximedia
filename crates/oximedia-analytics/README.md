# oximedia-analytics

Media engagement analytics — viewer behavior, A/B testing, retention curves, and engagement scoring for OxiMedia

[![Crates.io](https://img.shields.io/crates/v/oximedia-analytics.svg)](https://crates.io/crates/oximedia-analytics)
[![Documentation](https://docs.rs/oximedia-analytics/badge.svg)](https://docs.rs/oximedia-analytics)
[![License](https://img.shields.io/crates/l/oximedia-analytics.svg)](LICENSE)

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) sovereign media framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- Viewer session tracking with play, pause, seek, buffer, quality-change, and end events
- Playback maps showing per-second content coverage and attention heatmaps
- Audience retention curves with configurable bucket granularity
- Drop-off detection, re-watch segment identification, and benchmark comparison (broadcast, VOD, short-form)
- A/B testing with deterministic FNV-1a variant assignment, weighted allocation, and two-proportion z-test significance analysis
- Weighted engagement scoring with watch-time, completion, rewatch, social, and forward-seek penalty components
- Linear regression trend analysis and content ranking by engagement
- Pure Rust with zero C/Fortran dependencies

## Quick Start

```toml
[dependencies]
oximedia-analytics = "0.2.0"
```

### Session Analysis

```rust
use oximedia_analytics::session::{ViewerSession, PlaybackEvent, analyze_session};

let mut session = ViewerSession::new("sess_1", None, "video_42", 0);
session.push_event(PlaybackEvent::Play { timestamp_ms: 0 });
session.push_event(PlaybackEvent::End {
    position_ms: 30_000,
    watch_duration_ms: 30_000,
});

let metrics = analyze_session(&session, 60_000);
assert_eq!(metrics.completion_pct, 50.0);
```

### Retention Curves

```rust
use oximedia_analytics::retention::{compute_retention, average_view_duration};
use oximedia_analytics::session::{ViewerSession, PlaybackEvent};

let sessions: Vec<ViewerSession> = make_sessions(); // your sessions
let curve = compute_retention(&sessions, 120_000, 10);
let avg_duration = average_view_duration(&curve);
```

### A/B Testing

```rust
use oximedia_analytics::ab_testing::{
    Experiment, Variant, ExperimentResults, assign_variant,
    AssignmentMethod, winning_variant, z_test, is_significant,
};

let experiment = Experiment {
    id: "thumb_test".to_string(),
    name: "Thumbnail A/B".to_string(),
    variants: vec![
        Variant { id: "A".to_string(), name: "Control".to_string(), allocation_weight: 1.0 },
        Variant { id: "B".to_string(), name: "New Design".to_string(), allocation_weight: 1.0 },
    ],
    start_ms: 0,
    end_ms: None,
    min_sample_size: 100,
};

// Deterministic assignment: same user always gets the same variant
let variant = assign_variant(&experiment, "user_123", AssignmentMethod::Deterministic).unwrap();

// Statistical significance via two-proportion z-test
let z = z_test(0.10, 5000, 0.05, 5000);
assert!(is_significant(z, 0.05));
```

### Engagement Scoring

```rust
use oximedia_analytics::engagement::{
    compute_engagement, EngagementWeights, ContentRanker,
};

let weights = EngagementWeights::default();
let score = compute_engagement(&sessions, 60_000, &weights);
// score.score is in [0.0, 1.0]
// score.components breaks down watch_time, completion, rewatch, etc.
```

## Modules

### `session`

Core playback event model (`PlaybackEvent`: Play, Pause, Seek, BufferStart, BufferEnd, QualityChange, End) and `ViewerSession` for tracking a single viewing session. `analyze_session` produces `SessionMetrics` with total watch time, unique positions, seek count, buffer events, quality changes, and completion percentage. `build_playback_map` reconstructs a boolean per-second `PlaybackMap` from event sequences. `attention_heatmap` aggregates multiple sessions into normalized `HeatPoint` values across configurable time buckets.

### `retention`

`compute_retention` builds a `RetentionCurve` from viewer sessions with configurable checkpoint granularity. `average_view_duration` computes the trapezoidal integral of the curve. `drop_off_points` identifies positions where retention drops sharply. `re_watch_segments` finds content segments watched more than once on average. Built-in benchmarks (`BROADCAST_BENCHMARK`, `VOD_BENCHMARK`, `SHORT_FORM_BENCHMARK`) and `compare_to_benchmark` produce a quality score in [0, 100] comparing actual retention against expected values at 25%, 50%, and 75% positions.

### `ab_testing`

`Experiment` and `Variant` model multi-arm experiments with weighted allocation. `assign_variant` uses FNV-1a hashing for deterministic, reproducible user-to-variant assignment. `ExperimentResults` and `VariantMetrics` track impressions, clicks, conversions, completions, and watch duration per variant. Rate helpers (`click_through_rate`, `conversion_rate`, `completion_rate`, `average_watch_duration`) compute per-variant statistics. `z_test` performs a two-proportion z-test for statistical significance at configurable alpha levels (0.05, 0.01). `winning_variant` selects the best variant by CTR, conversion, completion, or watch duration.

### `engagement`

`compute_engagement` produces a `ContentEngagementScore` with decomposed `EngagementComponents` (watch time, completion, rewatch, social, forward-seek penalty) weighted by `EngagementWeights`. `EngagementTrend` stores score time-series and `linear_regression_slope` computes the least-squares slope for trend analysis. `ContentRanker::rank_by_engagement` sorts content by score descending.

### `error`

`AnalyticsError` covering missing variants, invalid weights, and other analytics-specific failures.

## Architecture

Sessions are modeled as ordered event streams. All analytics functions are stateless and operate on slices of sessions, making them suitable for both real-time and batch processing. Playback maps provide the bridge between event-level data and position-level aggregation used by retention and heatmap computations. The A/B testing module uses FNV-1a hashing rather than an external PRNG to ensure deterministic, reproducible variant assignment with no additional dependencies.

## License

Licensed under the terms specified in the workspace root.

Copyright (c) COOLJAPAN OU (Team Kitasan)
