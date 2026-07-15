# oximedia-io

![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)
![Tests: 607](https://img.shields.io/badge/tests-607-brightgreen)
![Updated: 2026-07-14](https://img.shields.io/badge/updated-2026--07--14-blue)

> I/O layer for the OxiMedia multimedia framework.

Part of the [OxiMedia](https://github.com/cool-japan/oximedia) multimedia framework.

## Features

- **Magic-byte format detection** — identifies over 45 media formats (MP4, MKV, AVI, WebM, MXF, FLAC, WAV, PNG, JPEG, WebP, DPX, EXR, JPEG-XL, and more) from a leading-bytes buffer
- **Media sources** — unified async `MediaSource` trait with `FileSource` (tokio) and `MemorySource` (`bytes::Bytes`) implementations
- **Bit-level reading** — MSB-first `BitReader` for video bitstream parsing; unsigned and signed Exp-Golomb (`ue(v)` / `se(v)`) coding for H.264-style syntax
- **MXF probing** — lightweight parser for MXF Header Partition Pack, operational pattern, and essence tracks
- **Content detection** — text encoding (UTF-8, UTF-16, Latin-1) and binary-vs-text heuristics
- **Aligned I/O** — memory-aligned reads and writes for DMA-friendly transfers
- **Buffered I/O** — read-ahead buffering; synchronous buffered reader
- **Memory-mapped I/O** — zero-copy mmap-based file access
- **Scatter-gather I/O** — vectorized multi-buffer I/O
- **Checksums** — CRC32, CRC64, SHA-256, BLAKE3
- **Compression** — zstd, LZ4, gzip, bzip2 (compress/decompress)
- **Progress reader** — async reader with callback for upload/download progress
- **Rate limiter** — bandwidth-limited I/O
- **Ring buffer** — lock-free ring buffer for streaming pipelines
- **Retrying source** — automatic retry on transient I/O errors
- **Chunked writer** — write large outputs in fixed-size chunks
- **Copy engine** — high-throughput async file copy
- **Temp files** — secure temporary file creation and cleanup
- **Verification I/O** — read-back verification for write integrity
- **Write journal** — journaled writes for crash-safe I/O
- **File metadata** — extended file attributes (size, timestamps, permissions)
- **File watching** — file system event watching

## Usage

```toml
[dependencies]
oximedia-io = "0.2.0"
```

### Detect media format

```rust
use oximedia_io::format_detector::FormatDetector;

let header = std::fs::read("input.mkv").unwrap();
let detection = FormatDetector::detect(&header);
println!("format: {:?}", detection.format);  // MediaFormat::Mkv
println!("mime:   {}", detection.mime_type); // "video/x-matroska"
```

### Async file source

```rust
use oximedia_io::source::{FileSource, MediaSource};

#[tokio::main]
async fn main() -> oximedia_core::OxiResult<()> {
    let mut source = FileSource::open("video.webm").await?;
    let mut buf = [0u8; 4096];
    let n = source.read(&mut buf).await?;
    println!("read {n} bytes");
    Ok(())
}
```

### Bit-level parsing (H.264 SPS)

```rust
use oximedia_io::bits::BitReader;

let sps = [0x64u8, 0x00, 0x1f];  // profile_idc=100, flags=0, level_idc=31
let mut r = BitReader::new(&sps);
let profile_idc = r.read_bits(8).unwrap(); // 100 — High Profile
let _flags      = r.read_bits(6).unwrap();
let level_idc   = r.read_bits(8).unwrap(); // 31 — Level 3.1
assert_eq!(profile_idc, 100);
assert_eq!(level_idc,    31);
```

### Exp-Golomb coding

```rust
use oximedia_io::bits::BitReader;

// ue(0) is encoded as a single `1` bit
let data = [0b10000000u8];
let mut r = BitReader::new(&data);
assert_eq!(r.read_exp_golomb().unwrap(), 0);
```

## API Overview

| Type | Description |
|------|-------------|
| `MediaSource` | Unified async read trait |
| `FileSource` | Tokio async file reader (non-WASM) |
| `MemorySource` | Zero-copy in-memory reader |
| `BitReader` | MSB-first bit-level reader |
| `FormatDetector` | Magic-byte media format identifier |
| `FormatDetection` | Detection result (format, MIME type, extension, confidence) |
| `MxfInfo` | MXF container probe result |

## Status

Alpha — API may change between minor versions.

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
