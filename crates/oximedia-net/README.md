# oximedia-net

**Status: [Stable]** | Version: 0.2.1 | Tests: extensively tested | Updated: 2026-08-12

Network streaming protocols for the OxiMedia multimedia framework.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

## Overview

`oximedia-net` provides implementations of network streaming protocols with adaptive bitrate, CDN integration, live streaming, and professional broadcast transport.

| Protocol | Description |
|----------|-------------|
| HLS | HTTP Live Streaming (Apple) |
| DASH | Dynamic Adaptive Streaming over HTTP |
| RTMP | Real-Time Messaging Protocol |
| SRT | Secure Reliable Transport |
| WebRTC | Real-time browser communication |
| SMPTE ST 2110 | Professional media over IP |
| RTP/RTSP | Real-time Transport Protocol |
| IP Multicast | Multicast streaming |

## Features

### HLS (HTTP Live Streaming)

- Master playlist parsing and generation
- Media playlist parsing and live segment tracking
- Variant stream selection
- Segment URL extraction and fetching

```rust
use oximedia_net::hls::{MasterPlaylist, MediaPlaylist};

let playlist = MasterPlaylist::parse(&m3u8_content)?;
for variant in playlist.variants() {
    println!("Variant: {}x{} @ {} bps",
        variant.width, variant.height, variant.bandwidth);
}
```

### DASH (MPEG-DASH)

- MPD (Media Presentation Description) parsing
- Period and adaptation set handling
- Segment template support
- Live/VOD and DVR window management

```rust
use oximedia_net::dash::MpdParser;

let mpd = MpdParser::parse(&xml_content)?;
for period in mpd.periods() {
    println!("Period: {:?}", period.duration);
}
```

### RTMP (Real-Time Messaging Protocol)

- Handshake protocol (C0/C1/C2, S0/S1/S2)
- Chunk stream handling
- AMF0/AMF3 encoding/decoding
- Message types (audio, video, command)
- RTMP client for publishing and playing

```rust
use oximedia_net::rtmp::{RtmpClient, RtmpConfig};

let config = RtmpConfig::new()
    .with_app("live")
    .with_stream_key("stream123");

let mut client = RtmpClient::connect("rtmp://server/app", config).await?;
client.publish(&packet).await?;
```

### SRT (Secure Reliable Transport)

- Packet structure and parsing
- Handshake (caller/listener/rendezvous)
- Congestion control (TLPKTDROP)
- AES encryption support
- Connection monitoring and statistics
- Key exchange

### WebRTC

- ICE candidate handling and ICE agent
- STUN/TURN support
- SDP parsing and generation
- DataChannel support
- DTLS transport
- SRTP/SRTCP
- RTCP
- SCTP

### SMPTE ST 2110

- Video transport (ST 2110-20)
- Audio transport (ST 2110-30)
- Ancillary data (ST 2110-40)
- PTP/IEEE 1588 timing
- RTP stream management
- SDP generation

### Live Streaming

- HLS live server
- DASH live server
- Live stream analytics

### CDN Integration

- CDN failover
- CDN metrics and monitoring

## API Overview

**Core types:**
- `NetError` / `NetResult` — Error handling
- `ConnectionPool` — HTTP connection pooling

**Modules:**
- `abr` — Adaptive bitrate control
- `bandwidth_estimator`, `bandwidth_throttle` — Bandwidth estimation and throttling
- `cdn` — CDN integration (failover, metrics)
- `connection_pool` — HTTP/TCP connection pooling
- `dash` — DASH streaming (client, MPD, segments, live DVR/chunked)
- `error` — Error types
- `flow_control` — Flow control
- `hls` — HLS streaming (playlist, segments)
- `live` — Live streaming servers (HLS, DASH) and analytics
- `multicast` — IP multicast support
- `network_path` — Network path analysis
- `packet_buffer` — Packet buffering
- `protocol_detect` — Protocol auto-detection
- `qos_monitor` — Quality of service monitoring
- `retry_policy` — Retry and backoff policies
- `rtmp` — RTMP protocol (handshake, chunk, AMF, client, message)
- `rtp_session` — RTP session management
- `session_tracker` — Session tracking
- `smpte2110` — SMPTE ST 2110 professional IP media transport
- `srt` — SRT protocol (crypto, key exchange, packet, stream, monitor)
- `stream_mux` — Stream multiplexing
- `webrtc` — WebRTC (ICE, STUN, SDP, DTLS, SRTP, DataChannel, SCTP)

## Policy

- No unsafe code
- No warnings (clippy pedantic)
- Apache 2.0 license

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
