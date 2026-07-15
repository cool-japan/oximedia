# oximedia-container

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Container format mux/demux for the OxiMedia multimedia framework — MP4, MKV, MPEG-TS, OGG, and more.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-container` provides demuxers and muxers for media container formats:

| Format     | Demux | Mux | Extensions              |
|------------|-------|-----|-------------------------|
| Matroska   | Yes   | Yes | .mkv                    |
| WebM       | Yes   | Yes | .webm                   |
| MPEG-TS    | Yes   | Yes | .ts, .m2ts              |
| MP4        | Yes   | —   | .mp4 (AV1/VP9 only)     |
| Ogg        | Yes   | —   | .ogg, .opus, .oga       |
| FLAC       | Yes   | —   | .flac                   |
| WAV        | Yes   | —   | .wav                    |

## Features

- **Format Detection** — Automatic format probing from magic bytes
- **Matroska/WebM** — Full EBML parser, cluster management, cue points, chapters, attachments
- **MPEG-TS** — PES packet parsing, PAT/PMT tables, PCR/timing, DVB support
- **MP4** — Atom-based parsing, edit lists, sample tables, timecode tracks, fragments (CMAF)
- **Ogg** — Page-based demuxing for Vorbis, Opus, FLAC, Theora
- **Metadata** — Tag editing for Vorbis comments, Matroska tags, MP4 metadata
- **Streaming** — Streaming demux and mux for live workflows
- **Seeking** — Keyframe-accurate seeking with cue/index structures
- **Tracks** — Multi-track management: video, audio, subtitle, data (GPS, telemetry)
- **Chapters** — Chapter list reading and writing (Matroska and MP4)
- **Timecode** — SMPTE timecode track support

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-container = "0.2.0"
```

### Format Detection

```rust
use oximedia_container::probe_format;

let data = std::fs::read("video.mkv")?;
let result = probe_format(&data)?;
println!("Format: {:?}, Confidence: {:.1}%",
    result.format, result.confidence * 100.0);
```

### Demuxing

```rust
use oximedia_container::{demux::MatroskaDemuxer, Demuxer};
use oximedia_io::FileSource;

let source = FileSource::open("video.mkv").await?;
let mut demuxer = MatroskaDemuxer::new(source);
demuxer.probe().await?;

for stream in demuxer.streams() {
    println!("Stream {}: {:?}", stream.index, stream.codec);
}

while let Ok(packet) = demuxer.read_packet().await {
    println!("Packet: stream={}, size={}, keyframe={}",
             packet.stream_index, packet.size(), packet.is_keyframe());
}
```

### Muxing

```rust
use oximedia_container::mux::{MatroskaMuxer, Muxer, MuxerConfig};

let config = MuxerConfig::new().with_title("My Video");
let mut muxer = MatroskaMuxer::new(sink, config);
muxer.add_stream(video_info)?;
muxer.add_stream(audio_info)?;
muxer.write_header().await?;

for packet in packets {
    muxer.write_packet(&packet).await?;
}
muxer.write_trailer().await?;
```

### Metadata Editing

```rust
use oximedia_container::metadata::MetadataEditor;

let mut editor = MetadataEditor::open("audio.flac").await?;
if let Some(title) = editor.get_text("TITLE") {
    println!("Title: {}", title);
}
editor.set("TITLE", "New Title");
editor.save().await?;
```

## Key Types

| Type | Description |
|------|-------------|
| `ContainerFormat` | Enumeration of supported formats |
| `Packet` | Compressed media packet with timestamps |
| `PacketFlags` | Packet properties (keyframe, corrupt, etc.) |
| `StreamInfo` | Stream metadata (codec, dimensions, etc.) |
| `CodecParams` | Codec-specific parameters |
| `Demuxer` | Trait for container demuxers |
| `Muxer` | Trait for container muxers |
| `Mp4FragmentMode` | Progressive vs. fragmented (CMAF/DASH) MP4 mux mode |
| `DecodeSkipCursor` | Sample-accurate seek result (keyframe offset + skip count) |
| `CmafChunkMode` | Standard / Chunked / LowLatencyChunked CMAF delivery mode |
| `CmafChunkedConfig` | Full configuration for CMAF chunked transfer |
| `BlockAdditionMapping` | Matroska v4 per-track auxiliary data channel descriptor |

## Wave 4 Additions (0.1.4)

### `Mp4FragmentMode` — Progressive and Fragmented MP4

`mux::mp4::Mp4FragmentMode` selects how the MP4 muxer lays out sample data:

- **`Progressive`** (default) — classic single-`moov` + `mdat` file, best for file download.
- **`Fragmented { fragment_duration_ms }`** — ISOBMFF fragmented MP4 (fMP4); each fragment is
  an independent `moof` + `mdat` pair suitable for DASH and CMAF delivery.

`Mp4Mode` is a backward-compatible type alias for `Mp4FragmentMode`.

### `DecodeSkipCursor` — Sample-Accurate Seeking

`seek_sample_accurate()` on the Matroska, MP4, and AVI demuxers returns a `DecodeSkipCursor`
that pinpoints where to start decoding (`byte_offset`, `sample_index`) and how many decoded
samples to discard (`skip_samples`) before the requested `target_pts` is reached.

### `CmafChunkMode` / `CmafChunkedConfig` — CMAF Low-Latency Delivery

`streaming::mux::CmafChunkMode` implements ISO/IEC 23000-19 chunked CMAF transfer:

| Mode | Description |
|------|-------------|
| `Standard` | Whole segments (no chunking; default) |
| `Chunked` | Multiple `moof`+`mdat` pairs per segment, governed by `chunk_duration_ms` |
| `LowLatencyChunked` | One sample per chunk — minimum end-to-end latency |

`CmafChunkedConfig::signal_low_latency` causes the muxer to write `cmfl` in the `styp` box
compatible-brands list, signalling LL-CMAF compliance to players.

### `BlockAdditionMapping` — Matroska v4 HDR/DV Track Metadata

`demux::matroska::matroska_v4::BlockAdditionMapping` (EBML ID `0x41CB`) carries auxiliary
per-block data channels on a Matroska track — used for HDR10+ dynamic metadata, Dolby Vision
RPU payloads, depth maps, and similar extensions.  Accessible via
`StreamInfo::block_addition_mappings` after probing.

## Module Structure (95 source files, 951 public items)

```
src/
├── lib.rs              # Crate root
├── demux/
│   ├── matroska/       # EBML parser, Matroska/WebM demuxer
│   ├── mpegts/         # MPEG-TS packet, PES, PAT/PMT
│   ├── ogg/            # Ogg page demuxer
│   ├── flac/           # FLAC demuxer
│   ├── wav/            # WAV demuxer
│   └── mp4/            # MP4/MOV demuxer
├── mux/
│   ├── matroska/       # Matroska/WebM muxer (EBML writer, clusters, cues)
│   └── mpegts/         # MPEG-TS muxer (PES, PCR)
├── metadata/           # Tag editing (Vorbis, Matroska, MP4)
├── chapters/           # Chapter list handling
├── fragment/           # Fragmented MP4 / CMAF support
├── streaming/          # Streaming demux and mux
├── tracks/             # Multi-track management
├── seek/               # Seeking infrastructure
├── timecode/           # SMPTE timecode track
├── data/               # Data tracks (GPS, telemetry, atoms)
├── edit/               # Edit list handling
└── cue/                # Cue point generation and optimization
```

## Patent Policy

All supported codecs are royalty-free:

**Supported**: AV1, VP9, VP8, Theora, Opus, Vorbis, FLAC, PCM

**Rejected**: H.264, H.265, AAC, AC-3, DTS (returns `PatentViolation` error if detected)

## Policy

- No unsafe code (`#![forbid(unsafe_code)]`)
- Apache-2.0 license

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
