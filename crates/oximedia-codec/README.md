# oximedia-codec

![Status: Mixed (see decoder matrix)](https://img.shields.io/badge/status-mixed-yellow)

Video and audio codec implementations for the OxiMedia multimedia framework. Pure-Rust, royalty-free codecs with image I/O support.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Overview

`oximedia-codec` provides encoding and decoding for royalty-free video codecs plus image I/O.
Decoder rows use the project-wide four-tier honesty taxonomy; see
[`docs/codec_status.md`](../../docs/codec_status.md) for per-decoder details, what is missing
per codec, and the effort required to close each gap.

| Label | Meaning |
|-------|---------|
| **Verified** | End-to-end decode matches a reference implementation on external fixtures. |
| **Functional** | Real reconstruction path present and self-consistent on round-trip tests. No third-party conformance proof yet. |
| **Bitstream-parsing** | Headers/syntax parsed; pixel/sample production is stubbed, partial, or returns empty/constant data. Useful for format inspection, not for playback. |
| **Experimental** | API sketch; not intended to decode. |

| Codec    | Encode     | Decode              | Feature Flag      | Notes |
|----------|------------|---------------------|-------------------|-------|
| AV1      | Functional | **Functional** (keyframe/intra only) | `av1` (default)   | Keyframe/intra decode real, bit-exact vs dav1d/aomdec (8-bit 4:2:0 profile 0), incl. deblocking/CDEF/loop restoration; inter-frame decode, super-resolution, film grain, palette mode, intra block copy, quantizer matrices, 10/12-bit not yet implemented (honest `Err`). GitHub issue #9. |
| VP9      | Functional | **Functional** (keyframe/intra only) | `vp9`             | Keyframe/intra decode real, bit-exact vs libvpx (8-bit 4:2:0); inter-frame decode not yet implemented (honest `Err`). |
| VP8      | Functional | **Functional** (keyframe/intra only) | `vp8`             | Full RFC 6386 §11–§15 keyframe/intra pipeline, bit-exact vs libwebp; inter-frame decode not yet implemented (honest `Err`). |
| Theora   | Functional | Bitstream-parsing   | `theora`          | DCT, motion compensation, and per-frame pixel hand-off into `VideoFrame` (decode hand-off bug fixed in 0.1.8, issue #9); encoder↔decoder bitstream alignment for full round-trip remains outstanding. |
| H.263    | Functional | Functional          | *(always)*        | Real macroblock decode, motion compensation, loop filter. |
| MJPEG    | Functional | Functional          | `mjpeg`           | Wraps `oximedia-image` JPEG baseline; ≥28 dB PSNR at Q85. |
| APV      | Functional | Functional          | `apv`             | ISO/IEC 23009-13 royalty-free intra-frame. |
| FFV1     | Functional | Functional          | `ffv1`            | RFC 9043 lossless; CRC-32 verified. |
| Opus     | Functional | Functional (CELT + SILK + Hybrid) | `opus` | RFC 6716 §4.2 SILK + §4.5 Hybrid decode real and wired (not stubs); no bit-exact libopus conformance fixtures yet. |
| Vorbis   | Functional | Bitstream-parsing   | *(always)*        | Headers parse; `decode_audio_packet` returns an honest `Err` (not fabricated empty samples). |
| FLAC     | Functional | Functional / Verified | *(always)*      | CRC-16 verified; real LPC decode. |
| PCM      | Verified   | Verified            | *(always)*        | Trivial round-trip verified. |
| JPEG-XL  | Functional | Functional          | `jpegxl`          | ISOBMFF container + streaming decode; real modular decoder. |
| PNG/APNG | Functional | Functional          | *(always)*        | Real unfilter + RGBA conversion. |
| WebP     | Functional | Functional (VP8L only) | *(always)*     | Lossless only — no VP8 lossy decoder. |
| GIF      | Functional | Functional          | *(always)*        | Real LZW decode. |
| AVIF     | Functional | Bitstream-parsing   | *(always)*        | Container validates; `decode()` returns an honest `Err` — not yet wired to the new AV1 keyframe/intra decoder. |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-codec = "0.2.0"
# or with additional codecs:
oximedia-codec = { version = "0.2.0", features = ["av1", "vp9", "vp8", "opus"] }
```

### AV1 / VP9 / VP8 / Theora Bitstream Parsing

These decoders currently parse bitstreams but do not provide a
self-consistent end-to-end round-trip (see the matrix above). They are
useful for format inspection, extradata handling, and container-side
work. The API shape below matches what the crate exposes today; the
returned frames have allocated planes but do not contain
externally-validated reconstructed pixels until the work tracked in
GitHub issue #9 lands. (Theora's decoder no longer drops the
reconstructed pixels into a temporary `to_vec()` clone — the 0.1.8
hand-off fix lands them in `VideoFrame.planes[i].data` — but the
encoder↔decoder bitstream contract for non-trivial inputs is still
outstanding.)

```rust,ignore
use oximedia_codec::{Av1Decoder, DecoderConfig, VideoDecoder};
use oximedia_core::CodecId;

let config = DecoderConfig {
    codec: CodecId::Av1,
    extradata: None,
    threads: 0,
    low_latency: false,
};
let mut decoder = Av1Decoder::new(config)?;
decoder.send_packet(&packet_bytes, pts)?;
while let Some(_frame) = decoder.receive_frame()? {
    // Frame metadata (width/height/format/timestamp) is populated;
    // pixel planes are not reconstructed in 0.1.8.
}
```

`Vp9Decoder`, `Vp8Decoder`, and `TheoraDecoder` follow the same
`VideoDecoder` trait shape. As of 0.1.8, `TheoraDecoder` lands its
per-block reconstructed pixels in `VideoFrame.planes[i].data` (rather
than dropping them into a temporary `Vec` clone), but full encode→decode
round-trip is still gated on a separate bitstream-alignment fix. See
`docs/codec_status.md` for the honest per-decoder status.

### Opus Decoding

```rust
use oximedia_codec::opus::OpusDecoder;

let mut decoder = OpusDecoder::new(48000, 2)?;
let audio_frame = decoder.decode_packet(&packet_data)?;
```

## JPEG-XL: ISOBMFF Container Output

`AnimatedJxlEncoder` (feature `jpegxl`) supports two output modes:

### `finish()` — bare codestream

Produces a raw JPEG-XL codestream starting with the `0xFF 0x0A` magic bytes.

### `finish_isobmff()` — ISOBMFF container

Wraps the codestream in the standard ISOBMFF box structure:

```
ftyp  (major brand: "jxl ", compatible: ["jxl ", "isom"])
jxll  (JXL level 5)
jxlp  (codestream packet with is_last flag set)
```

The resulting bytes are decodable by `JxlStreamingDecoder`:

```rust
# // no_run — requires jpegxl feature and runtime data
use std::io::Cursor;
// let bytes = encoder.finish_isobmff()?;
// let decoder = oximedia_codec::jpegxl::JxlStreamingDecoder::new(Cursor::new(bytes));
// for frame_result in decoder? { ... }
```

### Streaming Decode — `JxlStreamingDecoder<R: Read>`

`JxlStreamingDecoder` is a lazy `Iterator<Item = CodecResult<JxlFrame>>` that yields frames
one at a time without buffering the entire sequence. It auto-detects the format:

| Detection bytes | Format | Producer |
|-----------------|--------|----------|
| `[4..8] == b"ftyp"` and `[8..12] == b"jxl "` | ISOBMFF container | `finish_isobmff()` |
| `[0..2] == [0xFF, 0x0A]` | Native bare codestream | `finish()` |

```rust
# // no_run — requires jpegxl feature and runtime data
// use oximedia_codec::jpegxl::JxlStreamingDecoder;
// use std::io::Cursor;
//
// for frame_result in JxlStreamingDecoder::new(Cursor::new(data))? {
//     let frame = frame_result?;
//     println!("{}x{} ticks={}", frame.width, frame.height, frame.duration_ticks);
// }
```

## MJPEG: Baseline JPEG Spec Compliance

The `mjpeg` module (feature `mjpeg`) wraps `oximedia-image`'s pure-Rust JPEG baseline encoder
and decoder. The encoder:

- Emits DQT (Define Quantization Table) segments with quantization values in the standard
  JPEG zigzag scan order
- Achieves ≥28 dB PSNR at quality setting 85 for natural images (verified by round-trip tests)
- Produces fully self-contained JFIF-compliant JPEG frames suitable for AVI and MP4 containers

## Architecture

### Unified Traits

All codecs implement unified traits:

```rust
pub trait VideoDecoder {
    fn send_packet(&mut self, packet: &EncodedPacket) -> CodecResult<()>;
    fn receive_frame(&mut self) -> CodecResult<Option<VideoFrame>>;
    fn flush(&mut self) -> CodecResult<()>;
}

pub trait VideoEncoder {
    fn send_frame(&mut self, frame: &VideoFrame) -> CodecResult<()>;
    fn receive_packet(&mut self) -> CodecResult<Option<EncodedPacket>>;
    fn flush(&mut self) -> CodecResult<Vec<EncodedPacket>>;
}
```

### Rate Control Modes

| Mode | Description |
|------|-------------|
| CQP  | Constant QP — fixed quantization |
| CRF  | Constant Rate Factor — perceptual quality |
| CBR  | Constant Bitrate — fixed bitrate target |
| VBR  | Variable Bitrate — quality with bitrate limits |

### SIMD Support

The codec includes a SIMD abstraction layer:
- Scalar fallback (always available)
- SSE/AVX support (x86/x64)
- NEON support (ARM)

## Module Structure (194 source files, 3046 public items)

```
src/
├── lib.rs              # Crate root with re-exports
├── error.rs            # CodecError and CodecResult
├── frame.rs            # VideoFrame, Plane, ColorInfo
├── traits.rs           # VideoDecoder, VideoEncoder traits
├── av1/                # AV1 codec (OBU parsing, symbol coding, entropy)
├── vp9/                # VP9 codec (frame, context)
├── vp8/                # VP8 codec (DCT, motion, loop filter)
├── theora/             # Theora codec (VP3-based)
├── opus/               # Opus audio (SILK, CELT, range decoder)
├── intra/              # Shared intra prediction
├── motion/             # Motion estimation
├── rate_control/       # Rate control framework
├── reconstruct/        # Reconstruction pipeline
├── entropy_coding/     # Entropy coding
├── tile_encoder/       # Tile-based encoding
└── simd/               # SIMD abstraction (scalar, SSE/AVX, NEON)
```

## Patent Policy

All codecs are royalty-free:

**Supported codecs**: AV1, VP9, VP8, Theora, Opus

**Rejected codecs**: H.264, H.265, AAC, AC-3, DTS

When a patent-encumbered codec is detected in a container, a `PatentViolation` error is returned.

## Policy

- No unsafe code (`#![deny(unsafe_code)]`)
- Apache-2.0 license

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
