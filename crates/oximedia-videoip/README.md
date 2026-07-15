# oximedia-videoip

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Professional video-over-IP protocol for OxiMedia — a patent-free alternative to NDI for professional video streaming over IP networks.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Low-latency Streaming** - Target latency < 16ms at 60fps (less than 1 frame)
- **Patent-free Codecs** - VP9, AV1, VP8 (compressed), v210, UYVY (uncompressed)
- **Professional Audio** - Up to 16 channels at 48kHz/96kHz, Opus or PCM
- **Network Resilience** - Reed-Solomon FEC, jitter buffering, packet loss recovery
- **mDNS Discovery** - Automatic source detection via DNS-SD
- **Professional Features** - Tally lights, PTZ control, timecode, metadata
- **Multi-stream Support** - Program, preview, alpha channels
- **Bandwidth Estimation** - Adaptive bandwidth management
- **Network Bonding** - Multiple NIC aggregation for redundancy
- **Congestion Control** - Dynamic rate adaptation
- **Encryption** - Secure transport with configurable cipher suites
- **Flow Monitoring** - Per-stream flow statistics and health tracking
- **Frame Pacing** - Precise frame delivery timing
- **Multicast** - Efficient one-to-many streaming
- **NDI Bridge** - Interoperability bridge to NDI environments
- **NMOS** - IS-04/IS-05 NMOS discovery and connection management
- **PTP Boundary** - Precision Time Protocol boundary clock integration
- **QUIC Transport** - QUIC-based reliable transport option
- **RIST** - Reliable Internet Stream Transport support
- **SDP** - Session Description Protocol for stream negotiation
- **SMPTE 2110** - ST 2110 compliant streaming mode
- **SRT** - Secure Reliable Transport configuration
- **Stream Synchronization** - Multi-stream lip sync and A/V alignment

## Architecture

```
┌─────────────┐                           ┌─────────────┐
│  VideoIP    │  ─── UDP + FEC ────>      │  VideoIP    │
│  Source     │  <── Control Msgs ──      │  Receiver   │
└─────────────┘                           └─────────────┘
      │                                          │
      └──── mDNS Announcement                   │
                                                 │
                                 mDNS Discovery ─┘
```

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-videoip = "0.2.0"
```

### Broadcasting a Video Stream

```rust
use oximedia_videoip::{VideoIpSource, VideoConfig, AudioConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let video_config = VideoConfig::new(1920, 1080, 60.0)?;
    let audio_config = AudioConfig::new(48000, 2)?;

    let mut source = VideoIpSource::new("Camera 1", video_config, audio_config).await?;
    source.start_broadcasting()?;
    source.enable_fec(0.1)?; // 10% FEC overhead

    loop {
        let video_frame = capture_video_frame();
        let audio_samples = capture_audio_samples();
        source.send_frame(video_frame, Some(audio_samples)).await?;
    }
}
```

### Receiving a Video Stream

```rust
use oximedia_videoip::VideoIpReceiver;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut receiver = VideoIpReceiver::discover("Camera 1").await?;
    receiver.start_receiving();
    receiver.enable_fec(20, 2)?; // 20 data shards, 2 parity shards

    loop {
        let (video_frame, audio_samples) = receiver.receive_frame().await?;
        process_frame(video_frame, audio_samples);
    }
}
```

## Protocol Specification

### Packet Format

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Magic (OXVP)                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Version |Flags|   Sequence    |                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+                               +
|                         Timestamp (64-bit)                    |
+                               +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                               | Stream| Rsv |  Payload Size   |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         Payload Data                          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Discovery Protocol

Service type: `_oximedia-videoip._udp.local.`

TXT records: `codec`, `width`, `height`, `fps`, `audio_codec`, `sample_rate`, `channels`

## Performance

- Bitrate: Configurable (10 Mbps - 1 Gbps)
- Latency: < 16ms at 60fps
- Packet loss recovery: Up to 20% with FEC
- Jitter tolerance: Adaptive buffer (5-100ms)
- Max resolution: 8K UHD (7680x4320)
- Audio channels: Up to 16 channels

## API Overview

- `VideoIpSource` — Broadcast source with mDNS announcement, FEC, tally, and PTZ control
- `VideoIpReceiver` — Discovery and reception with jitter buffering and FEC decoding
- `VideoConfig` — Resolution, frame rate, codec selection
- `AudioConfig` — Sample rate, channel count, codec selection
- `VideoFormat` / `AudioFormat` — Format enumerations
- `VideoIpError` / `VideoIpResult` — Error and result types
- Modules: `bandwidth_est`, `bonding`, `codec`, `congestion`, `discovery`, `encryption`, `error`, `fec`, `flow_monitor`, `flow_stats`, `frame_pacing`, `jitter`, `metadata`, `multicast`, `multicast_group`, `ndi_bridge`, `nmos`, `packet`, `ptp_boundary`, `ptz`, `quic_transport`, `receiver`, `redundancy`, `rist`, `sdp`, `smpte2110`, `source`, `srt_config`, `stats`, `stream_descriptor`, `stream_health`, `stream_sync`, `tally`, `transport`, `types`, `utils`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
