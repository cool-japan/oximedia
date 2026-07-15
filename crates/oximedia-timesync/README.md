# oximedia-timesync

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Precision time synchronization for OxiMedia. Provides comprehensive time synchronization including PTP (IEEE 1588-2019), NTP, timecode synchronization, and media-specific genlock for professional broadcast and production environments.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **PTP (IEEE 1588-2019)** - Precision Time Protocol with sub-microsecond accuracy, Ordinary Clock, Boundary Clock, and Transparent Clock
- **Best Master Clock Algorithm (BMCA)** - Automatic grandmaster selection with unicast and multicast modes
- **NTP (RFC 5905)** - Network Time Protocol v4 client with server pool management and automatic failover
- **LTC Timecode** - Linear Timecode generation and jam sync with SMPTE 12M compliance
- **MTC (MIDI Timecode)** - MIDI Time Code support
- **Clock Discipline** - PID controller for smooth adjustments
- **Drift Compensation** - Drift prediction and holdover mode for maintaining accuracy without reference
- **Genlock** - Reference genlock generation for video sync
- **Frame-accurate Video Sync** - Video frame synchronization
- **Sample-accurate Audio Sync** - Audio sample clock synchronization
- **AES67** - AES67 audio-over-IP timing
- **gPTP (IEEE 802.1AS)** - Generalized PTP for media networks
- **Shared Memory IPC** - Microsecond-level clock access via Unix domain socket and shared memory
- **Clock Ensemble** - Multi-source clock selection and weighting
- **Clock Recovery** - Clock recovery from media streams
- **Frequency Estimation** - Precision frequency measurement and tracking
- **Jitter Buffer** - Adaptive jitter buffering for media streams
- **Leap Second** - Leap second announcement and handling
- **Phase Lock Loop** - PLL-based clock steering
- **Sync Audit** - Synchronization quality audit trail
- **Sync Metrics** - Detailed synchronization statistics and monitoring

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-timesync = "0.2.0"
```

```rust
use oximedia_timesync::{ClockDiscipline, ClockSource};
use oximedia_timesync::ntp::NtpClient;

// NTP synchronization
let mut client = NtpClient::new();
let result = client.synchronize().await?;
println!("NTP offset: {:.6} s", result.offset);

// Clock discipline
let mut discipline = ClockDiscipline::new();
let adjustment = discipline.update(10_000, ClockSource::Ptp)?;
```

```rust
use oximedia_timesync::{ClockIdentity, Domain};
use oximedia_timesync::ptp::clock::OrdinaryClock;

// PTP ordinary clock
let clock_id = ClockIdentity::random();
let mut clock = OrdinaryClock::new(clock_id, Domain::DEFAULT);
clock.bind("0.0.0.0:319".parse()?).await?;
let offset = clock.offset_from_master();
println!("Offset from master: {} ns", offset);
```

## API Overview

- `ClockIdentity` / `Domain` / `PortIdentity` / `PtpTimestamp` / `CommunicationMode` / `DelayMechanism` — PTP identifiers and configuration
- `NtpClient` / `NtpPacket` / `NtpTimestamp` / `ServerPool` / `Stratum` — NTP client
- `ClockDiscipline` / `DriftEstimator` / `HoldoverManager` / `OffsetFilter` / `SourceSelector` — Clock control
- `ClockSource` / `SyncState` / `ClockStats` / `SyncMode` — Clock status and synchronization
- `TimecodeState` / `TimecodeSource` — Timecode synchronization
- `GenlockGenerator` / `GenlockFrameRate` — Genlock generation
- `VideoSync` / `FrameAccurateSync` / `AudioSync` — Media clock synchronization
- `TimestampSync` / `adjust_timestamp` / `system_time_to_timestamp` — Integration with oximedia-core
- `StateInfo` / `TimeSyncMessage` — IPC message types
- `TimeSyncError` / `TimeSyncResult` — Error and result types
- Modules: `aes67`, `boundary_clock`, `clock`, `clock_discipline`, `clock_domain`, `clock_ensemble`, `clock_error`, `clock_recovery`, `clock_steering`, `dante_clock`, `drift_monitor`, `error`, `ffi`, `frequency_estimator`, `frequency_sync`, `gptp`, `holdover_estimator`, `integration`, `ipc`, `jitter_buffer`, `leap_second`, `ntp`, `offset_correction`, `offset_filter`, `phase_lock`, `ptp`, `reference_clock`, `sync`, `sync_audit`, `sync_metrics`, `sync_monitor`, `sync_protocol`, `sync_stats`, `sync_status`, `sync_window`, `time_reference`, `timecode`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
