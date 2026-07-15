# oximedia-automation

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional broadcast automation and control system for 24/7 operation with Lua scripting.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-automation` provides comprehensive broadcast automation with master control orchestration, multi-channel playout, device control, failover, emergency alert system (EAS) integration, and remote monitoring.

## Features

### Master Control
- Centralized orchestration of all automation subsystems
- Multi-channel support: manage multiple simultaneous broadcast channels
- System state management and comprehensive logging
- As-run logs and event logging for compliance

### Channel Automation
- Frame-accurate playlist execution
- Automated production switcher control
- Pre-roll management for seamless transitions
- Event triggers for automated responses

### Device Control
- **VDCP Protocol** — Video Disk Control Protocol; frame: `[STX=0x02][LEN][CMD][DATA][CHK][ETX=0x03]`; wrapping-add checksum
- **Sony 9-pin** — RS-422 VTR control via tokio-serial; 7-byte frame (`SONY_CMD1_TRANSPORT=0x20`); Stop/Play/Record/FF/Rewind
- **GPI/GPO** — General Purpose Interface triggers and outputs with debounce
- **Serial Communication** — Abstracted serial port interface

### Failover and Redundancy
- Hot standby with automatic failover
- Proactive system health monitoring
- Frame-accurate failover transitions
- Multi-channel coordinated failover

### Emergency Alert System (EAS)
- Alert processing (tornado, flood, earthquake, and more)
- Automated scrolling text crawl generation
- Attention tones and text-to-speech audio insertion
- Automatic alert prioritization

### Live Production
- Automated switcher and router control
- Layer-based graphics overlay management
- Automated source cycling sequences

### System Monitoring
- Real-time CPU, memory, disk, and network metrics
- Continuous system health assessment
- Anomaly detection and performance trending

### Remote Control
- REST API via axum for remote control (HTTP/JSON)
- WebSocket for real-time bidirectional communication
- Secure authentication and multi-client support

### Scripting (opt-in, C-backed — `lua-scripting` feature, off by default)
- Embedded Lua 5.4 scripting engine (mlua, vendored) — **not** part of the default,
  Pure Rust build; enable with `features = ["lua-scripting"]`
- Sandbox limits: 1 M instructions, 32 MiB memory, 5 s max duration
- FIFO script cache of 64 entries
- Comprehensive automation API exposed to scripts
- Custom workflows and event handlers
- Without the feature, `ScriptEngine` still exists with the same API but every
  Lua-dependent method returns a clear `Err` instead of executing anything

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-automation = "0.2.0"
```

### Basic Example

```rust
use oximedia_automation::{MasterControl, MasterControlConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = MasterControlConfig {
        num_channels: 2,
        eas_enabled: true,
        remote_enabled: true,
        ..Default::default()
    };

    let mut master = MasterControl::new(config).await?;
    master.start().await?;

    let status = master.status().await?;
    println!("System status: {:?}", status);

    Ok(())
}
```

### Playlist Execution

```rust
use oximedia_automation::playlist::{PlaylistExecutor, ExecutableItem};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut executor = PlaylistExecutor::new().await?;

    let item = ExecutableItem {
        id: "item001".to_string(),
        file_path: "/content/show_001.mxf".to_string(),
        duration_frames: 86400,  // 1 hour at 24fps
        scheduled_start: None,
        preroll_frames: 150,
        metadata: HashMap::new(),
    };

    executor.enqueue(item).await?;
    executor.start().await?;
    Ok(())
}
```

### EAS Alert Handling

```rust
use oximedia_automation::eas::{EasAlert, EasAlertType};
use std::time::Duration;

let mut alert = EasAlert::new(
    EasAlertType::TornadoWarning,
    "Tornado warning for Jefferson County".to_string(),
    Duration::from_secs(1800),
);
alert.add_location("039173".to_string()); // FIPS code
master.handle_alert(alert).await?;
```

### Lua Scripting

Requires the opt-in `lua-scripting` feature (`features = ["lua-scripting"]`);
without it, `ScriptEngine::execute` returns `Err` instead of running the script.

```rust
use oximedia_automation::script::ScriptEngine;

let mut engine = ScriptEngine::new()?;
engine.load_api()?;

let script = r#"
    automation.log("Starting automated sequence")
    automation.play()
    automation.cut()
    automation.log("Sequence complete")
"#;
engine.execute(script)?;
```

## Architecture

```
Master Control
    ├── Channel Automation (per channel)
    │   ├── Playlist Executor (PlaylistArena bump allocator) + Pre-roll Manager
    │   ├── Device Controllers (VDCP, Sony 9-pin, GPI, GPO)
    │   └── Live Switcher
    └── Shared Services
        ├── Failover Manager (Health Monitor, Failover Switch)
        ├── EAS System (EasPlayoutController, Crawl Generator, Audio Insertion)
        ├── Logging (BatchedAsRunLogger, Event Logger)
        ├── Monitoring (SystemMonitor, Metrics Collector, HttpSessionPool)
        ├── Remote Control (REST API via axum, WebSocket)
        └── Script Engine (Lua 5.4 via mlua, sandboxed; opt-in `lua-scripting` feature)
```

## Safety

This crate uses `#![forbid(unsafe_code)]` to ensure memory safety.

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
