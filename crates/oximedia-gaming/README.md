# oximedia-gaming

![Status: Stable](https://img.shields.io/badge/status-stable-green)
![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)

Game streaming and screen capture optimization for OxiMedia, providing ultra-low latency game streaming, capture, replay, highlights, and platform integration.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Ultra-Low Latency Streaming** — <100ms glass-to-glass latency for responsive streaming
- **Screen Capture** — Monitor, window, and region capture with multi-monitor support
- **Frame Pacing** — Smooth frame pacing for consistent streaming cadence
- **Input Visualization** — Keyboard, mouse, and controller input overlay display
- **Controller Mapping** — Configurable controller button mapping display
- **Webcam Integration** — Picture-in-picture with chroma key and auto-framing
- **Audio Mixing** — Multi-source audio (game, microphone, music)
- **Overlay System** — Stream alerts, chat widgets, scoreboard overlays
- **Scene Management** — Multiple scene switching with smooth transitions
- **Replay Buffer** — Instant replay of last 30–120 seconds
- **Highlight Detection** — Auto-detect gaming highlights (kills, wins, achievements)
- **Clip Manager** — Clip recording, trimming, and management
- **VOD Manager** — Video on demand archiving
- **Performance HUD** — Real-time FPS, CPU/GPU usage, encoding latency display
- **Stream Analytics** — Live stream analytics and viewer metrics
- **Network Quality** — Network quality monitoring and adaptive bitrate
- **Event Timeline** — Game event timeline tracking
- **Tournament Support** — Tournament bracket and metadata integration
- **Platform Integration** — Twitch, YouTube Gaming, Facebook Gaming, custom RTMP
- **Monetization** — Donation alerts and subscription events
- **Spectator Mode** — Spectator view management
- **Game Profiles** — Optimized profiles per game genre (FPS, MOBA, fighting, racing, RPG)
- **Safety** — `#![forbid(unsafe_code)]`

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-gaming = "0.2.0"
```

```rust
use oximedia_gaming::{GameStreamer, StreamConfig, CaptureSource, EncoderPreset};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = StreamConfig::builder()
        .source(CaptureSource::PrimaryMonitor)
        .resolution(1920, 1080)
        .framerate(60)
        .encoder_preset(EncoderPreset::UltraLowLatency)
        .bitrate(6000)
        .replay_buffer(30)
        .build()?;

    let mut streamer = GameStreamer::new(config).await?;
    streamer.start().await?;

    // Save instant replay
    streamer.save_replay("epic_moment.mp4").await?;

    streamer.stop().await?;
    Ok(())
}
```

## API Overview

**Core types:**
- `GameStreamer` — Main game streaming engine
- `StreamConfig` — Stream configuration builder
- `CaptureSource` — Monitor / Window / Region capture source
- `EncoderPreset` — UltraLowLatency / LowLatency / Quality presets
- `GameProfile` — Per-genre optimized streaming profiles

**Modules:**
- `capture`, `capture_config` — Screen capture and configuration
- `encode` — Encoding pipeline
- `stream_config` — Stream configuration
- `stream_overlay` — Stream overlay composition
- `stream_analytics` — Live analytics
- `overlay` — Overlay system
- `scene` — Scene management
- `audio` — Multi-source audio mixing
- `webcam` — Webcam integration
- `input`, `input_latency` — Input visualization and latency
- `controller_mapping` — Controller button mapping
- `replay` — Replay buffer management
- `clip_manager` — Clip recording and management
- `vod_manager` — VOD archiving
- `highlight` — Highlight detection
- `frame_pacing`, `pacing` — Frame pacing control
- `perf_hud` — Performance HUD overlay
- `network_quality` — Network quality monitoring
- `event_timeline` — Game event timeline
- `game_event` — Game event types
- `game_metadata` — Game metadata
- `player_stats` — Player statistics
- `session_stats` — Session statistics
- `leaderboard` — Leaderboard integration
- `achievement` — Achievement tracking
- `tournament` — Tournament support
- `chat_integration` — Chat platform integration
- `monetization` — Monetization events
- `spectator_mode` — Spectator view management
- `recording_profile` — Recording quality profiles
- `platform`, `platform_config` — Platform-specific integration
- `metrics` — Internal metrics

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
