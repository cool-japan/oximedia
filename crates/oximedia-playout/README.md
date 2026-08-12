# oximedia-playout

**Status: [Stable]** | Version: 0.2.1 | Tests: extensively tested | Updated: 2026-08-12

Professional broadcast playout server with frame-accurate timing, 24/7 reliability, and comprehensive output support.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Features

### Core Capabilities
- **Frame-accurate timing** - No dropped frames with sub-millisecond precision
- **24/7 reliability** - Emergency fallback and automatic recovery
- **Genlock support** - Professional broadcast synchronization
- **Low latency** - <100ms output latency
- **No unsafe code** - Memory-safe implementation (except hardware FFI)

### Scheduling & Playout
- **Advanced scheduler** - Time-based and frame-accurate triggering
- **Program templates** - Daily, weekly, monthly, and custom recurrence patterns
- **SCTE-35 support** - Ad insertion markers and splice commands
- **Cue points** - Frame-accurate markers for automation
- **Macro expansion** - Complex scheduling operations
- **Transition handling** - Cut, dissolve, fade, and wipe effects

### Playlist Management
- **Multiple formats** - SMIL, XML, JSON, M3U8
- **Dynamic insertion** - On-the-fly playlist modifications
- **Ad insertion** - Pre-roll, mid-roll, post-roll support
- **Loop modes** - Once, loop, shuffle, random fill
- **Fill content** - Automatic gap filling
- **Playlist ingest** - Format detection, item validation, clip trimming

### Playback Engine
- **Real-time playback** - Frame-accurate output
- **Clock synchronization** - Internal, SDI, PTP, NTP, Genlock
- **Buffer management** - Configurable buffering with underrun detection
- **Dropout handling** - Automatic recovery from signal loss
- **Emergency fallback** - Seamless switching to fallback content
- **Catchup** - Catch-up TV support

### Output Formats
- **SDI** - Blackmagic Decklink (hardware support)
- **NDI** - Network Device Interface
- **RTMP** - Live streaming (YouTube, Facebook, etc.)
- **SRT** - Secure Reliable Transport with encryption
- **SMPTE ST 2110** - Uncompressed IP video
- **SMPTE ST 2022** - Compressed IP with FEC
- **File output** - MXF, MP4, etc.

### Graphics Overlay
- **Logo/bug insertion** - Station branding
- **Lower thirds** - Name/title graphics
- **Character generator** - Full-screen text
- **Ticker/crawler** - News tickers and crawlers
- **Alpha blending** - Transparency support
- **Animations** - Fade, move, scale, rotate with easing curves

### Monitoring & Alerting
- **On-air status** - Real-time playout information
- **Next-up display** - Upcoming content preview
- **Audio meters** - Peak, RMS, loudness (LUFS)
- **Waveform/Vectorscope** - Video signal analysis
- **Alert system** - Configurable severity levels
- **Performance metrics** - CPU, memory, network, disk usage
- **Health check API** - System status monitoring

### Professional Features
- **BXF support** - Broadcast Exchange Format
- **As-run logging** - Detailed playout audit trail
- **Compliance ingest** - Compliance-checked content ingest
- **Rundown** - News rundown management
- **Signal chain** - Ordered processing chain with bypass
- **Tally system** - Tally light integration
- **Timecode overlay** - Timecode burn-in for monitoring
- **Secondary events** - Secondary event triggering
- **Highlight automation** - Automated highlight detection
- **Output router** - Multi-output routing
- **Channel config** - Channel format registry (SD/HD/UHD)
- **Schedule block** - Time-blocked schedule management
- **Schedule slot** - Time-slot grid with booking
- **Playout schedule** - 24-hour playout schedule grid

## Usage

```rust
use oximedia_playout::{PlayoutServer, PlayoutConfig, VideoFormat};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create server configuration
    let mut config = PlayoutConfig::default();
    config.video_format = VideoFormat::HD1080p25;
    config.genlock_enabled = true;
    config.monitoring_enabled = true;

    // Create and start playout server
    let server = PlayoutServer::new(config).await?;
    server.start().await?;

    // Load playlist
    server.load_playlist("/var/oximedia/playlists/daily.json".into()).await?;

    // Wait for server to run
    server.wait().await?;

    Ok(())
}
```

## API Overview

**Core types:**
- `PlayoutServer` — Main server API
- `PlayoutConfig` — Configuration management
- `VideoFormat` — HD/UHD format definitions (HD1080p25, HD1080p2997, UHD2160p50, etc.)
- `AudioFormat` — Audio configuration (sample rate, channels, bit depth)
- `PlayoutState` — Server state (Stopped, Starting, Running, Paused, Fallback, Stopping)

**Modules:**
- `ad_insertion` — SCTE-35 ad insertion
- `api` — REST control API
- `asrun` — As-run log management
- `automation` — Automation engine
- `branding` — Station branding overlays
- `bxf` — Broadcast Exchange Format support
- `catchup` — Catch-up TV support
- `cg` — Character generator
- `channel`, `channel_config` — Channel configuration
- `clip_store` — Clip storage management
- `compliance_ingest` — Compliance-checked ingest
- `content` — Content management
- `device` — Hardware device management
- `event_log` — Event logging
- `failover` — Emergency failover
- `frame_buffer` — Frame ring buffer
- `gap_filler` — Gap detection and filling
- `graphics` — Graphics overlay engine
- `highlight_automation` — Highlight automation
- `ingest` — Content ingest
- `media_router_playout` — Signal routing
- `monitoring` — Monitoring and alerting
- `output`, `output_router` — Output management and routing
- `playback` — Playback engine
- `playlist`, `playlist_ingest` — Playlist management and ingest
- `playout_log` — Playout audit trail
- `playout_schedule` — 24-hour schedule grid
- `rundown` — News rundown
- `schedule_block`, `schedule_slot` — Schedule management
- `scheduler` — Scheduling engine
- `secondary_events` — Secondary event triggering
- `signal_chain` — Signal processing chain
- `tally_system` — Tally lights
- `timecode_overlay` — Timecode burn-in

## Performance

- **Target latency**: <100ms
- **Frame accuracy**: ±1 frame tolerance (configurable)
- **Buffer size**: Configurable (default: 10 frames)
- **Supported formats**: Up to UHD 4K @ 60fps
- **Audio**: Up to 48kHz/24-bit, multi-channel

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
